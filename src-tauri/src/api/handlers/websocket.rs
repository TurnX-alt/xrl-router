//! key_stats 广播 WebSocket：前端订阅密钥池状态变更。

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;

use crate::gateway::server::AppState;

pub(crate) async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(mut socket: WebSocket, state: Arc<AppState>) {
    let mut rx = state.key_stats_tx.subscribe();
    while let Ok(msg) = rx.recv().await {
        let text = msg.to_string();
        if socket.send(Message::Text(text.into())).await.is_err() {
            break;
        }
    }
}
