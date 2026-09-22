use balabol_audio::{list_audio_devices, AudioDeviceInfo, FilterMode};
use balabol_capture::{list_capture_sources, CaptureSource};
use balabol_engine::{BalabolEngine, ContactCard, NetworkStatusInfo, StoredRoom, RoomMemberCard};
use balabol_net::DiscoveredLanPeer;
use balabol_storage::{StoredChannel, StoredMessage};
use serde::Serialize;
use std::sync::Arc;
use tauri::State;


pub struct AppState {
    pub engine: Arc<BalabolEngine>,
}

#[derive(Serialize)]
pub struct UserInfo {
    pub user_id: String,
    pub public_key_hex: String,
    pub is_sfu_node: bool,
    pub invite_code: String,
}

#[derive(Serialize)]
pub struct AudioDevicesResponse {
    pub inputs: Vec<AudioDeviceInfo>,
    pub outputs: Vec<AudioDeviceInfo>,
}

#[tauri::command]
fn get_user_info(state: State<'_, AppState>) -> UserInfo {
    let engine = &state.engine;
    let is_sfu = engine.sfu_election.lock().is_current_node_sfu();
    UserInfo {
        user_id: engine.user_id(),
        public_key_hex: engine.public_key_hex(),
        is_sfu_node: is_sfu,
        invite_code: engine.get_my_invite_code(),
    }
}

#[tauri::command]
fn list_channels(state: State<'_, AppState>) -> Result<Vec<StoredChannel>, String> {
    state.engine.list_channels().map_err(|e| e.to_string())
}

#[tauri::command]
fn list_channels_for_room(room_id: String, state: State<'_, AppState>) -> Result<Vec<StoredChannel>, String> {
    state.engine.list_channels_for_room(&room_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_rooms(state: State<'_, AppState>) -> Result<Vec<StoredRoom>, String> {
    state.engine.list_rooms().map_err(|e| e.to_string())
}

#[tauri::command]
fn create_room(name: String, emoji: String, state: State<'_, AppState>) -> Result<StoredRoom, String> {
    state.engine.create_room(&name, &emoji).map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_room(room_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.engine.delete_room(&room_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn create_channel(
    name: String,
    channel_type: String,
    room_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<StoredChannel, String> {
    state.engine
        .create_channel(&name, &channel_type, room_id.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_channel(channel_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.engine.delete_channel(&channel_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_messages(
    channel_id: String,
    limit: Option<usize>,
    state: State<'_, AppState>,
) -> Result<Vec<StoredMessage>, String> {
    state.engine.get_messages(&channel_id, limit.unwrap_or(50)).map_err(|e| e.to_string())
}

#[tauri::command]
fn send_message(
    channel_id: String,
    content: String,
    state: State<'_, AppState>,
) -> Result<StoredMessage, String> {
    state.engine.send_message(&channel_id, &content).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_user_volume(
    user_id: String,
    volume: f32,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.engine.set_user_volume(&user_id, volume).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_user_volume(user_id: String, state: State<'_, AppState>) -> f32 {
    state.engine.get_user_volume(&user_id)
}

#[tauri::command]
fn set_filter_mode(mode: String, state: State<'_, AppState>) {
    let filter = match mode.as_str() {
        "DeepFilterNet" => FilterMode::DeepFilterNet,
        "NoiseGate" => FilterMode::NoiseGate,
        _ => FilterMode::Off,
    };
    state.engine.set_filter_mode(filter);
}

#[tauri::command]
fn set_mute(muted: bool, state: State<'_, AppState>) {
    state.engine.set_mute(muted);
}

#[tauri::command]
fn set_deafen(deafened: bool, state: State<'_, AppState>) {
    state.engine.set_deafen(deafened);
}

#[tauri::command]
fn list_screen_sources() -> Result<Vec<CaptureSource>, String> {
    list_capture_sources().map_err(|e| e.to_string())
}

#[tauri::command]
fn get_audio_devices() -> AudioDevicesResponse {
    let (inputs, outputs) = list_audio_devices();
    AudioDevicesResponse { inputs, outputs }
}

#[tauri::command]
fn start_voice_transmission(channel_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.engine.start_voice_channel(&channel_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn stop_voice_transmission(state: State<'_, AppState>) {
    state.engine.stop_voice_channel();
}

#[tauri::command]
fn set_echo_test(enabled: bool, state: State<'_, AppState>) {
    state.engine.set_echo_test(enabled);
}

#[tauri::command]
fn is_echo_test_active(state: State<'_, AppState>) -> bool {
    state.engine.is_echo_test_active()
}

#[tauri::command]
fn get_mic_level(state: State<'_, AppState>) -> f32 {
    state.engine.get_mic_level()
}

#[tauri::command]
fn get_lan_peers(state: State<'_, AppState>) -> Vec<DiscoveredLanPeer> {
    state.engine.get_lan_peers()
}

#[tauri::command]
fn connect_direct(endpoint_or_invite: String, state: State<'_, AppState>) -> Result<String, String> {
    state.engine.connect_peer_direct(&endpoint_or_invite).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_my_invite_code(state: State<'_, AppState>) -> String {
    state.engine.get_my_invite_code()
}

#[tauri::command]
fn get_network_status(state: State<'_, AppState>) -> NetworkStatusInfo {
    state.engine.get_network_status()
}

#[tauri::command]
fn list_contacts(state: State<'_, AppState>) -> Result<Vec<ContactCard>, String> {
    state.engine.list_contacts().map_err(|e| e.to_string())
}

#[tauri::command]
fn add_friend(
    name: String,
    invite_or_id: String,
    state: State<'_, AppState>,
) -> Result<ContactCard, String> {
    state.engine.add_friend(&name, &invite_or_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_contact(user_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.engine.delete_contact(&user_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_connected_peers(state: State<'_, AppState>) -> Vec<String> {
    state.engine.get_connected_peers()
}

#[tauri::command]
fn start_screen_share(channel_id: String, source_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.engine.start_screen_share(&channel_id, &source_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn stop_screen_share(state: State<'_, AppState>) {
    state.engine.stop_screen_share();
}

#[tauri::command]
fn get_latest_screen_frame(state: State<'_, AppState>) -> Option<String> {
    state.engine.get_latest_screen_frame()
}

#[tauri::command]
fn get_screen_streamer_id(state: State<'_, AppState>) -> Option<String> {
    state.engine.get_screen_streamer_id()
}

#[tauri::command]
fn invite_friend_to_room(room_id: String, friend_user_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.engine.invite_friend_to_room(&room_id, &friend_user_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_room_members(room_id: String, state: State<'_, AppState>) -> Result<Vec<RoomMemberCard>, String> {
    state.engine.list_room_members(&room_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_room_voice_participants(room_id: String, state: State<'_, AppState>) -> Vec<String> {
    state.engine.get_room_voice_participants(&room_id)
}

#[tauri::command]
fn join_room_voice(room_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.engine.join_room_voice(&room_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn leave_room_voice(room_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.engine.leave_room_voice(&room_id).map_err(|e| e.to_string())
}

pub fn run() {
    tracing_subscriber::fmt::init();

    let app_data_dir = std::env::current_dir().unwrap().join("balabol_data.db");
    let engine = BalabolEngine::init(None, app_data_dir).expect("Failed to initialize Balabol Engine");
    engine.ensure_network_started();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState { engine })
        .invoke_handler(tauri::generate_handler![
            get_user_info,
            list_rooms,
            create_room,
            delete_room,
            list_channels,
            list_channels_for_room,
            create_channel,
            delete_channel,
            get_messages,
            send_message,
            set_user_volume,
            get_user_volume,
            set_filter_mode,
            set_mute,
            set_deafen,
            list_screen_sources,
            get_audio_devices,
            start_voice_transmission,
            stop_voice_transmission,
            set_echo_test,
            is_echo_test_active,
            get_mic_level,
            get_lan_peers,
            connect_direct,
            get_my_invite_code,
            get_network_status,
            list_contacts,
            add_friend,
            delete_contact,
            get_connected_peers,
            start_screen_share,
            stop_screen_share,
            get_latest_screen_frame,
            get_screen_streamer_id,
            invite_friend_to_room,
            list_room_members,
            get_room_voice_participants,
            join_room_voice,
            leave_room_voice
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

