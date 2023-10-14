use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use native_tls::TlsConnector;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, tungstenite::WebSocket};
use tokio_tungstenite::{connect_async_tls_with_config, Connector};

use crate::constants::GIO_ENDPOINT;
use crate::state::{MoveCommand, SerializedMoveCommand};
use crate::utils::fuck_socketio;

#[derive(Deserialize, Debug)]
// ["game_start",{"playerIndex":0,"playerColors":[0,1],"replay_id":"BeUebWTx6","chat_room":"game_1696563438364jZuK7n9viljyuDxOAAGF","usernames":["redbot","Anonymous"],"teams":[1,2],"game_type":"custom","swamps":[],"lights":[],"options":{}},null]
pub struct GameStart {
    #[serde(rename = "playerIndex")]
    pub player_index: u8,
    pub replay_id: String,
}

#[derive(Debug)]
enum ServerUpdate {
    PreGameStart,
    GameStart(GameStart),
    GameUpdate(StateUpdate),
    GameWon,
    GameLost,
    GameOver,
    SetSID(String),
}

#[derive(Deserialize, Debug)]
pub struct PlayerScore {
    #[serde(rename = "i")]
    pub player_index: u8,

    #[serde(rename = "total")]
    pub army_count: u16,

    #[serde(rename = "tiles")]
    pub tile_count: u16,
}

#[derive(Deserialize, Debug)]
pub struct StateUpdate {
    pub map_diff: Vec<i16>,
    pub cities_diff: Vec<i16>,
    pub turn: u64,
    pub generals: Vec<i64>,
    pub scores: Vec<PlayerScore>,
}
pub struct GeneralsClient {
    tx: mpsc::UnboundedSender<String>,

    update_rx: mpsc::UnboundedReceiver<ServerUpdate>,

    move_id: u64,
}

pub enum LobbyType {
    Private(String),
    OneVOne,
}

impl GeneralsClient {
    pub async fn connect(userid: &String, username: &String, lobby: &LobbyType) -> Self {
        let (ws_stream, _) = connect_async_tls_with_config(
            GIO_ENDPOINT,
            None,
            false,
            Some(Connector::NativeTls(
                TlsConnector::builder()
                    .danger_accept_invalid_certs(true)
                    .build()
                    .unwrap(),
            )),
        )
        .await
        .unwrap();

        let (mut ws_tx, mut ws_rx) = ws_stream.split();

        let (tx, mut rx) = mpsc::unbounded_channel();

        // pipe rx into ws_tx
        tokio::spawn(async move {
            loop {
                let msg = rx.recv().await.unwrap();
                trace!("Sending message: {}", msg);
                ws_tx.send(Message::Text(msg)).await.unwrap();
            }
        });

        // pipe ws_rx into update_rx
        let (update_tx, mut update_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut sid_sent = false;
            while let Some(msg) = ws_rx.next().await {
                let msg = msg.unwrap();

                if !msg.is_text() {
                    warn!("Unknown message: {:?}", msg);
                    continue;
                }

                // split msg at first {
                let msg = msg.to_string();
                let payload = fuck_socketio(msg);

                if payload.is_none() {
                    warn!("fuck socketio idk");
                    continue;
                }
                let payload = payload.unwrap();

                // info!("Received message: {}", payload);
                let deser = serde_json::from_str::<serde_json::Value>(&payload).unwrap();
                if !sid_sent {
                    // its a map with the "sid" key, get it
                    let sid = deser
                        .as_object()
                        .unwrap()
                        .get("sid")
                        .unwrap()
                        .as_str()
                        .unwrap();
                    update_tx.send(ServerUpdate::SetSID(sid.to_string()));
                    sid_sent = true;
                    continue;
                }
                // deser should be a vec
                if let Some(msg) = deser.as_array() {
                    if let Some(kind) = msg[0].as_str() {
                        match kind {
                            "pre_game_start" => {
                                update_tx.send(ServerUpdate::PreGameStart).unwrap();
                            }
                            "game_start" => {
                                let data = msg[1].clone();
                                update_tx
                                    .send(ServerUpdate::GameStart(
                                        serde_json::from_value(data).unwrap(),
                                    ))
                                    .unwrap();
                            }
                            "game_update" => {
                                let data = msg[1].clone();
                                let update = serde_json::from_value::<StateUpdate>(data).unwrap();
                                update_tx.send(ServerUpdate::GameUpdate(update)).unwrap();
                            }
                            "game_won" => {
                                update_tx.send(ServerUpdate::GameWon).unwrap();
                            }
                            "game_lost" => {
                                update_tx.send(ServerUpdate::GameLost).unwrap();
                            }
                            "game_over" => {
                                update_tx.send(ServerUpdate::GameOver).unwrap();
                            }
                            e => {
                                warn!("Unknown message type: {}", e);
                            }
                        }
                    } else {
                        warn!("Unknown message type: {}", payload);
                    }
                } else {
                    warn!("Unknown message: {}", payload);
                }
            }
            warn!("ws_rx closed");
        });

        let sid_update = update_rx.recv().await;
        if let Some(ServerUpdate::SetSID(sid)) = sid_update {
            info!("Got sid: {}", sid);
            tx.send(format!("40{{\"sid\":\"{}\"}}", sid)).unwrap();
        } else {
            panic!("Expected sid update");
        }

        // send heartbeat
        let heartbeat_sender = tx.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                heartbeat_sender.send("3".to_string()).unwrap();
            }
        });

        let client = GeneralsClient {
            tx,
            update_rx,
            move_id: 1,
        };

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        match lobby {
            LobbyType::Private(gameid) => {
                // join_private
                client
                    .send(Value::Array(vec![
                        Value::String("join_private".to_owned()),
                        Value::String(gameid.clone()),
                        Value::String(userid.clone()),
                        Value::String(username.clone()),
                    ]))
                    .await;

                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                // set force start
                client
                    .send(Value::Array(vec![
                        Value::String("set_force_start".to_owned()),
                        Value::String(gameid.clone()),
                        Value::Bool(true),
                    ]))
                    .await;
            }
            LobbyType::OneVOne => {
                // join_1v1
                client
                    .send(Value::Array(vec![
                        Value::String("join_1v1".to_owned()),
                        Value::String(userid.clone()),
                        Value::String(username.clone()),
                    ]))
                    .await;
            }
        }

        client
    }

    async fn send(&self, message: impl Serialize) {
        let msg = format!("42{}", serde_json::to_string(&message).unwrap());
        self.tx.send(msg).unwrap();
    }

    pub async fn send_cmd(&mut self, cmd: SerializedMoveCommand) {
        self.send(cmd.to_json(self.move_id)).await;
        self.move_id += 1;
    }

    pub async fn clear_commands(&mut self) {
        self.send(Value::Array(vec![Value::String("clear_moves".to_owned())]))
            .await;
    }

    pub async fn wait_game_start(&mut self) -> GameStart {
        while let Some(update) = self.update_rx.recv().await {
            match update {
                ServerUpdate::GameStart(g) => {
                    info!("Game started");
                    return g;
                }
                ServerUpdate::PreGameStart => {
                    info!("Game pre-start");
                }
                _ => {
                    // warn!("Unexpected update: {:?}", update);
                }
            }
        }
        panic!("update_rx closed before game start");
    }

    pub async fn get_game_update(&mut self) -> Option<StateUpdate> {
        let mut res;

        loop {
            if let Some(update) = self.update_rx.recv().await {
                if let ServerUpdate::GameUpdate(g) = update {
                    res = g;
                    break;
                } else if let ServerUpdate::GameOver = update {
                    return None;
                } else {
                    warn!("Unexpected update: {:?}", update);
                }
            } else {
                panic!("update_rx closed before game update");
            }
        }
        // drain channel
        while let Ok(update) = self.update_rx.try_recv() {
            if let ServerUpdate::GameUpdate(g) = update {
                warn!("Skipped update!!");
                res = g;
            } else {
                warn!("Unexpected update: {:?}", update);
            }
        }
        Some(res)
    }
}
