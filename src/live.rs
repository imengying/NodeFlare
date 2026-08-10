use worker::*;

#[durable_object(websocket)]
pub struct LiveHub {
    state: State,
}

impl DurableObject for LiveHub {
    fn new(state: State, _env: Env) -> Self {
        Self { state }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        if req.method() == Method::Post {
            let payload = req.text().await?;
            let server_id = req.headers().get("X-Server-ID")?.unwrap_or_default();
            let mut sockets = self.state.get_websockets_with_tag("all");
            if !server_id.is_empty() {
                sockets.extend(
                    self.state
                        .get_websockets_with_tag(&format!("server:{server_id}")),
                );
            }
            for socket in sockets {
                if socket.send_with_str(&payload).is_err() {
                    let _ = socket.close(Some(1011), Some("send failed"));
                }
            }
            return Response::empty();
        }

        let is_upgrade = req
            .headers()
            .get("Upgrade")?
            .map(|value| value.eq_ignore_ascii_case("websocket"))
            .unwrap_or(false);
        if !is_upgrade {
            return Response::error("WebSocket upgrade required", 426);
        }

        let pair = WebSocketPair::new()?;
        let server_id = req
            .url()?
            .query_pairs()
            .find_map(|(key, value)| (key == "server_id").then(|| value.to_string()))
            .filter(|value| !value.is_empty() && value.len() <= 80 && !value.contains('/'));
        let tag = server_id.map_or_else(|| "all".to_string(), |id| format!("server:{id}"));
        self.state.accept_websocket_with_tags(&pair.server, &[&tag]);
        Response::from_websocket(pair.client)
    }

    async fn websocket_message(
        &self,
        ws: WebSocket,
        message: WebSocketIncomingMessage,
    ) -> Result<()> {
        if matches!(message, WebSocketIncomingMessage::String(ref value) if value == "ping") {
            ws.send_with_str("pong")?;
        }
        Ok(())
    }

    async fn websocket_close(
        &self,
        ws: WebSocket,
        code: usize,
        reason: String,
        _was_clean: bool,
    ) -> Result<()> {
        ws.close(Some(code as u16), Some(reason))
    }

    async fn websocket_error(&self, ws: WebSocket, _error: Error) -> Result<()> {
        ws.close(Some(1011), Some("socket error"))
    }
}

pub async fn broadcast(env: &Env, server_id: &str, payload: &str) -> Result<()> {
    let namespace = env.durable_object("LIVE_HUB")?;
    let stub = namespace.id_from_name("dashboard")?.get_stub()?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_body(Some(worker::wasm_bindgen::JsValue::from_str(payload)));
    let req = Request::new_with_init("https://live.internal/push", &init)?;
    req.headers().set("X-Server-ID", server_id)?;
    stub.fetch_with_request(req).await?;
    Ok(())
}

pub async fn upgrade(req: Request, env: &Env) -> Result<Response> {
    let namespace = env.durable_object("LIVE_HUB")?;
    let stub = namespace.id_from_name("dashboard")?.get_stub()?;
    stub.fetch_with_request(req).await
}
