mod invite;
mod logger;
mod p2p;
mod signaling;

use logger::AppLogger;
use p2p::{ChatMessage, ConnectionStatus, P2pNode, PeerInfo};
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
    state: State<'_, AppState>,
) -> Result<ChatMessage, String> {
    state.node.send_chat_message(text, nickname).await
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
fn get_logs(state: State<'_, AppState>) -> Vec<String> {
    state.logger.get_recent_logs()
}

#[tauri::command]
fn get_log_file_path(state: State<'_, AppState>) -> String {
    state.logger.log_file_path()
}

pub fn run() {
    let logger = AppLogger::init();
    logger.info("Запуск приложения Balabol P2P...");

    let node = Arc::new(P2pNode::new(logger.clone()));
    let app_state = AppState { node, logger };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            start_room,
            join_room,
            send_chat_message,
            get_chat_messages,
            disconnect,
            get_status,
            get_peers,
            get_logs,
            get_log_file_path,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
