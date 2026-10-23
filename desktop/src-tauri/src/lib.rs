pub mod audio;
pub mod crypto;
mod logger;
pub mod p2p;
pub mod screen_capture;
mod signaling;

use logger::AppLogger;
use p2p::{ChatMessage, ConnectionStatus, P2pNode, PeerInfo, RemoteVideoFrame, SignalingStatus, VoiceStatus};
use std::sync::Arc;
use tauri::State;

pub struct AppState {
    pub node: Arc<P2pNode>,
    pub logger: AppLogger,
}

#[tauri::command]
async fn start_room(
    room_code: String,
    nickname: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    state.node.start_room(room_code, nickname).await
}

#[tauri::command]
async fn join_room(
    room_code: String,
    nickname: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.node.join_room(room_code, nickname).await
}

#[tauri::command]
async fn send_chat_message(
    text: String,
    nickname: String,
    image_thumb: Option<String>,
    image_id: Option<String>,
    full_image_base64: Option<String>,
    state: State<'_, AppState>,
) -> Result<ChatMessage, String> {
    state
        .node
        .send_chat_message(text, nickname, image_thumb, image_id, full_image_base64)
        .await
}

#[tauri::command]
async fn request_full_image(image_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.node.request_full_image(image_id).await
}

#[tauri::command]
fn get_cached_image(image_id: String, state: State<'_, AppState>) -> Option<String> {
    state.node.get_cached_image(&image_id)
}

#[tauri::command]
async fn request_history(state: State<'_, AppState>) -> Result<(), String> {
    state.node.request_history().await;
    Ok(())
}

#[tauri::command]
fn get_chat_messages(state: State<'_, AppState>) -> Vec<ChatMessage> {
    state.node.get_chat_messages()
}

#[tauri::command]
fn disconnect(state: State<'_, AppState>) -> Result<(), String> {
    state.node.disconnect();
    Ok(())
}

#[tauri::command]
fn get_status(state: State<'_, AppState>) -> ConnectionStatus {
    state.node.get_status()
}

#[tauri::command]
fn get_peers(state: State<'_, AppState>) -> Vec<PeerInfo> {
    state.node.get_peers()
}

#[tauri::command]
fn get_signaling_status(state: State<'_, AppState>) -> SignalingStatus {
    state.node.get_signaling_status()
}

#[tauri::command]
fn reopen_signaling(state: State<'_, AppState>) -> Result<(), String> {
    state.node.reopen_signaling()
}

#[tauri::command]
fn get_logs(state: State<'_, AppState>) -> Vec<String> {
    state.logger.get_recent_logs()
}

#[tauri::command]
fn get_log_file_path(state: State<'_, AppState>) -> String {
    state.logger.log_file_path()
}

#[tauri::command]
fn set_mic_muted(muted: bool, state: State<'_, AppState>) {
    state.node.set_mic_muted(muted);
}

#[tauri::command]
fn set_deafened(deafened: bool, state: State<'_, AppState>) {
    state.node.set_deafened(deafened);
}

#[tauri::command]
fn set_master_volume(volume: f32, state: State<'_, AppState>) {
    state.node.set_master_volume(volume);
}

#[tauri::command]
fn get_master_volume(state: State<'_, AppState>) -> f32 {
    state.node.get_master_volume()
}

#[tauri::command]
fn set_peer_volume(peer_id: u32, volume: f32, state: State<'_, AppState>) {
    state.node.set_peer_volume(peer_id, volume);
}

#[tauri::command]
fn set_mic_sensitivity(sensitivity: f32, state: State<'_, AppState>) {
    state.node.set_mic_sensitivity(sensitivity);
}

#[tauri::command]
fn set_mic_gain(gain: f32, state: State<'_, AppState>) {
    state.node.set_mic_gain(gain);
}

#[tauri::command]
fn set_loopback_test(enabled: bool, state: State<'_, AppState>) {
    state.node.set_loopback_test(enabled);
}

#[tauri::command]
fn restart_audio(state: State<'_, AppState>) -> Result<(), String> {
    state.node.restart_audio()
}

#[tauri::command]
fn start_mic_test(state: State<'_, AppState>) -> Result<(), String> {
    state.node.start_mic_test()
}

#[tauri::command]
fn stop_mic_test(state: State<'_, AppState>) {
    state.node.stop_mic_test();
}

#[tauri::command]
fn get_voice_status(state: State<'_, AppState>) -> VoiceStatus {
    state.node.get_voice_status()
}

#[tauri::command]
fn get_remote_video_frame(last_frame_id: u32, state: State<'_, AppState>) -> Option<RemoteVideoFrame> {
    state.node.get_remote_video_frame(last_frame_id)
}

#[tauri::command]
async fn send_video_frame(
    jpeg_base64: String,
    is_screen: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.node.send_video_frame(jpeg_base64, is_screen).await
}

#[tauri::command]
async fn send_media_state(
    video_active: bool,
    screen_active: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.node.send_media_state(video_active, screen_active).await
}

#[tauri::command]
fn play_notification_sound(sound_type: String, state: State<'_, AppState>) {
    state.node.play_notification_sound(&sound_type);
}

#[tauri::command]
async fn transfer_host(new_host_id: u32, state: State<'_, AppState>) -> Result<(), String> {
    state.node.transfer_host(new_host_id).await
}

#[tauri::command]
async fn set_nickname(nickname: String, state: State<'_, AppState>) -> Result<(), String> {
    state.node.update_nickname(nickname).await
}

#[tauri::command]
async fn start_screen_capture(state: State<'_, AppState>) -> Result<(), String> {
    state.node.start_screen_capture().await
}

#[tauri::command]
async fn stop_screen_capture(state: State<'_, AppState>) -> Result<(), String> {
    state.node.stop_screen_capture().await
}

#[tauri::command]
fn set_hide_lan_mode(enabled: bool, state: State<'_, AppState>) {
    state.node.set_hide_lan_mode(enabled);
}

#[tauri::command]
fn get_hide_lan_mode(state: State<'_, AppState>) -> bool {
    state.node.is_hide_lan_mode()
}

pub fn run() {
    let logger = AppLogger::init();
    logger.info("Запуск приложения Balabol P2P...");

    let node = Arc::new(P2pNode::new(logger.clone()));
    let app_state = AppState { node: node.clone(), logger };

    let node_clone = node.clone();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            let handle = app.handle().clone();
            node_clone.set_video_frame_callback(move |frame| {
                use tauri::Emitter;
                let _ = handle.emit("video-frame", &frame);
                let _ = handle.emit("p2p://video_frame", &frame);
            });
            Ok(())
        })
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            start_room,
            join_room,
            send_chat_message,
            get_chat_messages,
            disconnect,
            get_status,
            get_peers,
            get_signaling_status,
            reopen_signaling,
            get_logs,
            get_log_file_path,
            set_mic_muted,
            set_deafened,
            set_master_volume,
            get_master_volume,
            set_peer_volume,
            set_mic_sensitivity,
            set_mic_gain,
            set_loopback_test,
            restart_audio,
            start_mic_test,
            stop_mic_test,
            play_notification_sound,
            get_voice_status,
            get_remote_video_frame,
            send_video_frame,
            send_media_state,
            transfer_host,
            set_nickname,
            request_full_image,
            get_cached_image,
            request_history,
            start_screen_capture,
            stop_screen_capture,
            set_hide_lan_mode,
            get_hide_lan_mode,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
