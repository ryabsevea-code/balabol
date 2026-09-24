use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfferData {
    pub name: String,
    pub room: String,
    pub candidates: Vec<SocketAddr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnswerData {
    pub name: String,
    pub candidates: Vec<SocketAddr>,
}

pub fn create_offer_link(name: &str, room: &str, candidates: Vec<SocketAddr>) -> String {
    let data = OfferData {
        name: name.to_string(),
        room: room.to_string(),
        candidates,
    };
    let json = serde_json::to_string(&data).unwrap_or_default();
    let encoded = URL_SAFE_NO_PAD.encode(json.as_bytes());
    format!("bala://offer/{}", encoded)
}

pub fn create_answer_link(name: &str, candidates: Vec<SocketAddr>) -> String {
    let data = AnswerData {
        name: name.to_string(),
        candidates,
    };
    let json = serde_json::to_string(&data).unwrap_or_default();
    let encoded = URL_SAFE_NO_PAD.encode(json.as_bytes());
    format!("bala://answer/{}", encoded)
}

pub fn parse_offer_link(raw_input: &str) -> Result<OfferData, String> {
    let trimmed = raw_input.trim();
    let token = if let Some(stripped) = trimmed.strip_prefix("bala://offer/") {
        stripped
    } else if let Some(stripped) = trimmed.strip_prefix("bala://") {
        stripped
    } else {
        trimmed
    };

    if let Ok(bytes) = URL_SAFE_NO_PAD.decode(token) {
        if let Ok(data) = serde_json::from_slice::<OfferData>(&bytes) {
            if !data.candidates.is_empty() {
                return Ok(data);
            }
        }
        // Fallback to legacy invite format
        #[derive(Deserialize)]
        struct LegacyInvite {
            name: String,
            room: String,
            loc: Option<String>,
            pub_addr: Option<String>,
        }
        if let Ok(legacy) = serde_json::from_slice::<LegacyInvite>(&bytes) {
            let mut c = Vec::new();
            if let Some(l) = legacy.loc {
                if let Ok(a) = l.parse() { c.push(a); }
            }
            if let Some(p) = legacy.pub_addr {
                if let Ok(a) = p.parse() { if !c.contains(&a) { c.push(a); } }
            }
            if !c.is_empty() {
                return Ok(OfferData {
                    name: legacy.name,
                    room: legacy.room,
                    candidates: c,
                });
            }
        }
    }

    if let Ok(addr) = trimmed.parse::<SocketAddr>() {
        return Ok(OfferData {
            name: "Прямой узел".into(),
            room: "Комната".into(),
            candidates: vec![addr],
        });
    }

    Err("Неверный формат Offer. Вставьте ссылку вида bala://offer/...".into())
}

pub fn parse_answer_link(raw_input: &str) -> Result<AnswerData, String> {
    let trimmed = raw_input.trim();
    let token = if let Some(stripped) = trimmed.strip_prefix("bala://answer/") {
        stripped
    } else if let Some(stripped) = trimmed.strip_prefix("bala://") {
        stripped
    } else {
        trimmed
    };

    if let Ok(bytes) = URL_SAFE_NO_PAD.decode(token) {
        if let Ok(data) = serde_json::from_slice::<AnswerData>(&bytes) {
            if !data.candidates.is_empty() {
                return Ok(data);
            }
        }
    }

    if let Ok(addr) = trimmed.parse::<SocketAddr>() {
        return Ok(AnswerData {
            name: "Узел-клиент".into(),
            candidates: vec![addr],
        });
    }

    Err("Неверный формат Answer. Вставьте ссылку вида bala://answer/...".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_offer_and_answer_roundtrip() {
        let candidates = vec![
            "192.168.1.192:9987".parse().unwrap(),
            "2.27.58.7:46801".parse().unwrap(),
        ];

        let offer_link = create_offer_link("HostPC", "Room1", candidates.clone());
        assert!(offer_link.starts_with("bala://offer/"));

        let offer = parse_offer_link(&offer_link).expect("Parse offer failed");
        assert_eq!(offer.name, "HostPC");
        assert_eq!(offer.room, "Room1");
        assert_eq!(offer.candidates.len(), 2);

        let answer_link = create_answer_link("LaptopClient", candidates.clone());
        assert!(answer_link.starts_with("bala://answer/"));

        let answer = parse_answer_link(&answer_link).expect("Parse answer failed");
        assert_eq!(answer.name, "LaptopClient");
        assert_eq!(answer.candidates.len(), 2);
    }
}
