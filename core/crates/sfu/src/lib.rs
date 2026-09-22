use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use thiserror::Error;
use tracing::{info, warn};

#[derive(Error, Debug)]
pub enum SfuError {
    #[error("Room not found: {0}")]
    RoomNotFound(String),
    #[error("User not subscribed to room")]
    UserNotSubscribed,
    #[error("Election failed: no eligible peers")]
    NoEligiblePeers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NatType {
    Open,           // Public IP or UPnP port mapped
    FullCone,       // Cone NAT: easy hole punching
    Restricted,     // Port-restricted cone NAT
    Symmetric,      // Symmetric NAT: cannot easily act as SFU router
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerScoreReport {
    pub user_id: String,
    pub avg_rtt_ms: f32,
    pub jitter_ms: f32,
    pub packet_loss_pct: f32,
    pub nat_type: NatType,
    pub last_ping_ms: u64,
}

impl PeerScoreReport {
    /// Calculate composite score where LOWER is BETTER.
    /// Symmetric NAT is penalized heavily (+10,000 points) because it cannot accept unsolicited UDP.
    pub fn composite_score(&self) -> f32 {
        let nat_penalty = match self.nat_type {
            NatType::Open => 0.0,
            NatType::FullCone => 20.0,
            NatType::Restricted => 80.0,
            NatType::Symmetric => 10000.0, // Ineligible for SFU unless no other choice
        };

        self.avg_rtt_ms + (self.jitter_ms * 2.5) + (self.packet_loss_pct * 50.0) + nat_penalty
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SfuHeartbeat {
    pub leader_id: String,
    pub term: u64,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverNotice {
    pub new_leader_id: String,
    pub term: u64,
    pub previous_leader_id: String,
    pub reason: String,
}

/// Raft-inspired dynamic leader election for P2P SFU supernodes.
/// Monitors peer latencies and automatically promotes the best available node.
pub struct SfuElectionManager {
    my_user_id: String,
    term: u64,
    current_leader: Option<String>,
    runner_up: Option<String>,
    last_heartbeat_ms: u64,
    peer_scores: HashMap<String, PeerScoreReport>,
}

impl SfuElectionManager {
    pub fn new(my_user_id: String) -> Self {
        Self {
            my_user_id,
            term: 1,
            current_leader: None,
            runner_up: None,
            last_heartbeat_ms: 0,
            peer_scores: HashMap::new(),
        }
    }

    pub fn current_leader(&self) -> Option<&String> {
        self.current_leader.as_ref()
    }

    pub fn runner_up(&self) -> Option<&String> {
        self.runner_up.as_ref()
    }

    pub fn is_current_node_sfu(&self) -> bool {
        self.current_leader.as_ref() == Some(&self.my_user_id)
    }

    /// Record updated latency/NAT metrics for a peer in the room.
    pub fn update_peer_score(&mut self, report: PeerScoreReport) {
        self.peer_scores.insert(report.user_id.clone(), report);
        self.recalculate_leaders();
    }

    /// Remove a peer when disconnected.
    pub fn remove_peer(&mut self, user_id: &str) {
        self.peer_scores.remove(user_id);
        if self.current_leader.as_deref() == Some(user_id) {
            warn!("Current SFU leader {} left room!", user_id);
            self.current_leader = None;
        }
        self.recalculate_leaders();
    }

    /// Recalculate ranking of best SFU candidates.
    pub fn recalculate_leaders(&mut self) {
        if self.peer_scores.is_empty() {
            return;
        }

        let mut sorted: Vec<&PeerScoreReport> = self.peer_scores.values().collect();
        sorted.sort_by(|a, b| {
            a.composite_score()
                .partial_cmp(&b.composite_score())
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let new_leader = sorted.first().map(|r| r.user_id.clone());
        let new_runner_up = sorted.get(1).map(|r| r.user_id.clone());

        self.current_leader = new_leader;
        self.runner_up = new_runner_up;
    }

    /// Record heartbeat incoming from the current SFU leader.
    pub fn handle_heartbeat(&mut self, heartbeat: SfuHeartbeat) {
        if heartbeat.term >= self.term {
            self.term = heartbeat.term;
            self.current_leader = Some(heartbeat.leader_id);
            self.last_heartbeat_ms = heartbeat.timestamp_ms;
        }
    }

    /// Check for SFU timeout. If no heartbeat received for > 1000ms, trigger failover!
    pub fn check_heartbeat_timeout(&mut self, current_time_ms: u64) -> Option<FailoverNotice> {
        // If current node is already the leader, no timeout needed
        if self.is_current_node_sfu() {
            return None;
        }

        // If we haven't received a heartbeat yet, start tracking
        if self.last_heartbeat_ms == 0 {
            self.last_heartbeat_ms = current_time_ms;
            return None;
        }

        let elapsed = current_time_ms.saturating_sub(self.last_heartbeat_ms);
        if elapsed > 1000 {
            // Failover threshold exceeded (1 second)
            let prev_leader = self.current_leader.clone().unwrap_or_else(|| "none".into());
            info!(
                "⚠️ SFU leader {} missed heartbeat ({}ms elapsed). Triggering failover!",
                prev_leader, elapsed
            );

            // Promote runner-up (node #2 in ping rankings)
            let new_leader = self.runner_up.clone().unwrap_or_else(|| self.my_user_id.clone());
            self.term += 1;
            self.current_leader = Some(new_leader.clone());
            self.last_heartbeat_ms = current_time_ms;

            // Remove former unresponsive leader from active candidate scores
            self.peer_scores.remove(&prev_leader);
            self.recalculate_leaders();

            return Some(FailoverNotice {
                new_leader_id: new_leader,
                term: self.term,
                previous_leader_id: prev_leader,
                reason: format!("Heartbeat timeout ({}ms > 1000ms)", elapsed),
            });
        }

        None
    }
}

/// In-memory Selective Forwarding Unit (SFU) router.
/// Receives 1 media packet from an active speaker and distributes it to N peers in the room
/// without transcribing, transcoding, or buffering delays.
pub struct SfuRouter {
    rooms: Arc<RwLock<HashMap<String, HashSet<String>>>>, // room_id -> Set<user_id>
}

impl SfuRouter {
    pub fn new() -> Self {
        Self {
            rooms: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Join a user to an SFU media room.
    pub fn join_room(&self, room_id: &str, user_id: &str) {
        let mut rooms = self.rooms.write();
        rooms
            .entry(room_id.to_string())
            .or_insert_with(HashSet::new)
            .insert(user_id.to_string());
    }

    /// Leave an SFU media room.
    pub fn leave_room(&self, room_id: &str, user_id: &str) {
        let mut rooms = self.rooms.write();
        if let Some(subscribers) = rooms.get_mut(room_id) {
            subscribers.remove(user_id);
            if subscribers.is_empty() {
                rooms.remove(room_id);
            }
        }
    }

    /// Resolve recipients for a media packet.
    /// Returns list of peer IDs that must receive this packet (all in room except the sender).
    pub fn route_media_packet(
        &self,
        room_id: &str,
        sender_id: &str,
    ) -> Result<Vec<String>, SfuError> {
        let rooms = self.rooms.read();
        let subscribers = rooms.get(room_id).ok_or_else(|| SfuError::RoomNotFound(room_id.to_string()))?;

        if !subscribers.contains(sender_id) {
            return Err(SfuError::UserNotSubscribed);
        }

        let recipients: Vec<String> = subscribers
            .iter()
            .filter(|&id| id != sender_id)
            .cloned()
            .collect();

        Ok(recipients)
    }

    /// Resolve recipients for a clustered multi-host room.
    /// Distinguishes between local participants and inter-host trunk connections.
    pub fn route_cluster_packet(
        &self,
        room_id: &str,
        sender_id: &str,
        is_inter_host_trunk: bool,
        other_hosts: &[String],
    ) -> Result<(Vec<String>, Vec<String>), SfuError> {
        let rooms = self.rooms.read();
        let subscribers = rooms.get(room_id).ok_or_else(|| SfuError::RoomNotFound(room_id.to_string()))?;

        // 1. Local clients to forward to (excluding sender and other hosts)
        let local_clients: Vec<String> = subscribers
            .iter()
            .filter(|&id| id != sender_id && !other_hosts.contains(id))
            .cloned()
            .collect();

        // 2. Other hosts to forward to (only if this packet was from a local client, not already from a trunk)
        let trunk_hosts: Vec<String> = if !is_inter_host_trunk {
            other_hosts.iter().filter(|&h| h != sender_id).cloned().collect()
        } else {
            Vec::new() // Do not bounce trunk packets back to other hosts to prevent loops
        };

        Ok((local_clients, trunk_hosts))
    }
}

impl SfuElectionManager {
    /// Elect multiple hosts for a large group room (e.g. 2 hosts if > 6 participants).
    pub fn elect_cluster_hosts(&self, max_hosts: usize) -> Vec<String> {
        let mut sorted: Vec<&PeerScoreReport> = self.peer_scores.values().collect();
        sorted.sort_by(|a, b| {
            a.composite_score()
                .partial_cmp(&b.composite_score())
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        sorted
            .into_iter()
            .take(max_hosts.max(1))
            .map(|r| r.user_id.clone())
            .collect()
    }

    /// Partition a participant to a specific host using deterministic hashing.
    pub fn assign_host_for_client(&self, client_id: &str, hosts: &[String]) -> Option<String> {
        if hosts.is_empty() {
            return None;
        }
        let hash: usize = client_id.bytes().map(|b| b as usize).sum();
        Some(hosts[hash % hosts.len()].clone())
    }
}


impl Default for SfuRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sfu_election_picks_lowest_ping_and_open_nat() {
        let mut election = SfuElectionManager::new("bala:user1".to_string());

        // Peer A: High ping, Open NAT -> score = 120.0
        election.update_peer_score(PeerScoreReport {
            user_id: "bala:peerA".to_string(),
            avg_rtt_ms: 120.0,
            jitter_ms: 5.0,
            packet_loss_pct: 0.0,
            nat_type: NatType::Open,
            last_ping_ms: 1000,
        });

        // Peer B: Low ping (15ms), Open NAT -> score = 15 + 2*2.5 = 20.0 (BEST)
        election.update_peer_score(PeerScoreReport {
            user_id: "bala:peerB".to_string(),
            avg_rtt_ms: 15.0,
            jitter_ms: 2.0,
            packet_loss_pct: 0.0,
            nat_type: NatType::Open,
            last_ping_ms: 1000,
        });

        // Peer C: Low ping (10ms) but Symmetric NAT (+10,000 penalty) -> score = 10010.0
        election.update_peer_score(PeerScoreReport {
            user_id: "bala:peerC".to_string(),
            avg_rtt_ms: 10.0,
            jitter_ms: 1.0,
            packet_loss_pct: 0.0,
            nat_type: NatType::Symmetric,
            last_ping_ms: 1000,
        });

        // Leader must be Peer B (best score), runner up must be Peer A (Peer C is disqualified by symmetric NAT)
        assert_eq!(election.current_leader().unwrap(), "bala:peerB");
        assert_eq!(election.runner_up().unwrap(), "bala:peerA");
    }

    #[test]
    fn test_sfu_failover_when_heartbeat_stops_for_1_second() {
        let mut election = SfuElectionManager::new("bala:peerA".to_string());

        election.update_peer_score(PeerScoreReport {
            user_id: "bala:peerB".to_string(),
            avg_rtt_ms: 10.0,
            jitter_ms: 1.0,
            packet_loss_pct: 0.0,
            nat_type: NatType::Open,
            last_ping_ms: 1000,
        });

        election.update_peer_score(PeerScoreReport {
            user_id: "bala:peerA".to_string(),
            avg_rtt_ms: 25.0,
            jitter_ms: 2.0,
            packet_loss_pct: 0.0,
            nat_type: NatType::Open,
            last_ping_ms: 1000,
        });

        assert_eq!(election.current_leader().unwrap(), "bala:peerB");
        assert_eq!(election.runner_up().unwrap(), "bala:peerA");

        // Receive initial heartbeat at t=1000ms
        election.handle_heartbeat(SfuHeartbeat {
            leader_id: "bala:peerB".to_string(),
            term: 1,
            timestamp_ms: 1000,
        });

        // At t=1500ms (500ms elapsed) -> no failover
        assert!(election.check_heartbeat_timeout(1500).is_none());

        // At t=2100ms (1100ms elapsed > 1000ms threshold) -> FAILOVER!
        let failover = election.check_heartbeat_timeout(2100).expect("Must trigger failover");
        assert_eq!(failover.new_leader_id, "bala:peerA");
        assert_eq!(failover.previous_leader_id, "bala:peerB");
        assert_eq!(election.current_leader().unwrap(), "bala:peerA");
    }

    #[test]
    fn test_sfu_router_packet_distribution() {
        let router = SfuRouter::new();
        router.join_room("room-gaming", "user1");
        router.join_room("room-gaming", "user2");
        router.join_room("room-gaming", "user3");

        let recipients = router.route_media_packet("room-gaming", "user1").unwrap();
        assert_eq!(recipients.len(), 2);
        assert!(recipients.contains(&"user2".to_string()));
        assert!(recipients.contains(&"user3".to_string()));
        assert!(!recipients.contains(&"user1".to_string())); // Don't echo to sender
    }

    #[test]
    fn test_multi_host_clustering() {
        let mut election = SfuElectionManager::new("bala:local".to_string());

        // Add 4 peers with different performance metrics
        election.update_peer_score(PeerScoreReport {
            user_id: "host1".into(),
            avg_rtt_ms: 12.0,
            jitter_ms: 0.5,
            packet_loss_pct: 0.0,
            nat_type: NatType::Open,
            last_ping_ms: 1000,
        });
        election.update_peer_score(PeerScoreReport {
            user_id: "host2".into(),
            avg_rtt_ms: 15.0,
            jitter_ms: 0.8,
            packet_loss_pct: 0.0,
            nat_type: NatType::Open,
            last_ping_ms: 1000,
        });
        election.update_peer_score(PeerScoreReport {
            user_id: "clientA".into(),
            avg_rtt_ms: 45.0,
            jitter_ms: 3.0,
            packet_loss_pct: 0.5,
            nat_type: NatType::FullCone,
            last_ping_ms: 1000,
        });
        election.update_peer_score(PeerScoreReport {
            user_id: "clientB".into(),
            avg_rtt_ms: 80.0,
            jitter_ms: 5.0,
            packet_loss_pct: 1.0,
            nat_type: NatType::Restricted,
            last_ping_ms: 1000,
        });

        // Elect 2 cluster hosts
        let hosts = election.elect_cluster_hosts(2);
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0], "host1");
        assert_eq!(hosts[1], "host2");

        // Test cluster router trunk
        let router = SfuRouter::new();
        router.join_room("room-clustered", "host1");
        router.join_room("room-clustered", "host2");
        router.join_room("room-clustered", "clientA");
        router.join_room("room-clustered", "clientB");

        // When clientA speaks on host1:
        let (local, trunks) = router
            .route_cluster_packet("room-clustered", "clientA", false, &hosts)
            .unwrap();
        assert!(local.contains(&"clientB".to_string()));
        assert!(trunks.contains(&"host2".to_string()));
    }
}

