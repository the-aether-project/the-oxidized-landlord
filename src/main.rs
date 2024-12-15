#[macro_use]
extern crate rocket;
use rocket::State;
use std::sync::Arc;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_VP8};
use webrtc::api::{APIBuilder, API};
use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
use webrtc::track::track_local::{TrackLocal, TrackLocalWriter};
use webrtc::util::Conn;

use rocket::fairing::{Fairing, Info, Kind};
use rocket::http::Header;
use rocket::{Request, Response};
struct AetherWebRTCConnectionManager {
    peer_connections: Vec<Arc<RTCPeerConnection>>,
    screen_track: Option<Arc<TrackLocalStaticRTP>>,
    rtc_configuration: RTCConfiguration,
    api: API,
}

const LOCAL_RTP_ADDR: &str = "127.0.0.1:6421";

impl AetherWebRTCConnectionManager {
    fn default() -> Option<Self> {
        let mut m = MediaEngine::default();

        m.register_default_codecs().ok()?;

        let mut registry = Registry::new();

        registry = register_default_interceptors(registry, &mut m).ok()?;

        let api = APIBuilder::new()
            .with_media_engine(m)
            .with_interceptor_registry(registry)
            .build();

        Some(Self {
            peer_connections: vec![],
            screen_track: None,
            rtc_configuration: RTCConfiguration {
                ice_servers: vec![RTCIceServer {
                    urls: vec!["stun:stun.l.google.com:19302".to_owned()],
                    ..Default::default()
                }],
                ..Default::default()
            },
            api: api,
        })
    }

    async fn connect(
        &mut self,
        offer: RTCSessionDescription,
    ) -> anyhow::Result<RTCSessionDescription> {
        if let Some(screen_track) = &self.screen_track {
            todo!("Implement broadcast system.")
        } else {
            let track = TrackLocalStaticRTP::new(
                RTCRtpCodecCapability {
                    mime_type: MIME_TYPE_VP8.to_owned(),
                    ..Default::default()
                },
                "video".to_owned(),
                "aether-rtc-screen".to_owned(),
            );

            self.screen_track = Some(Arc::new(track));
        }

        // At this point, screen_track cannot be expected to be None.
        let track = self.screen_track.clone().unwrap();

        let peer = Arc::new(
            self.api
                .new_peer_connection(self.rtc_configuration.clone())
                .await?,
        );

        let rtp_sender = peer
            .add_track(Arc::clone(&track) as Arc<dyn TrackLocal + Send + Sync>)
            .await?;

        // TODO: Add this upon connection!
        self.peer_connections.insert(0, Arc::clone(&peer));

        // Handle incoming packets, if any.
        tokio::spawn(async move {
            let mut rtcp_buf = vec![0u8; 1500];
            while let Ok((_, _)) = rtp_sender.read(&mut rtcp_buf).await {}
            anyhow::Result::<()>::Ok(())
        });

        let (done_tx, mut done_rx) = tokio::sync::mpsc::channel::<()>(1);
        let done_tx1 = done_tx.clone();
        let done_tx2 = done_tx.clone();
        let done_tx3 = done_tx.clone();

        peer.on_ice_connection_state_change(Box::new(
            move |connection_state: RTCIceConnectionState| {
                match connection_state {
                    RTCIceConnectionState::Failed
                    | RTCIceConnectionState::Disconnected
                    | RTCIceConnectionState::Closed => {
                        let _ = done_tx1.try_send(());
                    }
                    _ => {}
                }

                Box::pin(async {})
            },
        ));

        peer.on_peer_connection_state_change(Box::new(
            move |connection_state: RTCPeerConnectionState| {
                match connection_state {
                    RTCPeerConnectionState::Failed
                    | RTCPeerConnectionState::Disconnected
                    | RTCPeerConnectionState::Closed => {
                        let _ = done_tx2.try_send(());
                    }
                    _ => {}
                }

                Box::pin(async {})
            },
        ));

        peer.set_remote_description(offer).await?;

        let answer = peer.create_answer(None).await?;
        let mut gather_complete = peer.gathering_complete_promise().await;

        peer.set_local_description(answer).await?;
        let _ = gather_complete.recv().await;

        // We expect a local description to be set here.
        // Thereforce, we safely unwrap.
        let answer = peer.local_description().await.unwrap();

        // Currently, this part gets executed for each peer which may be
        // detrimental to the server performance.

        // Not to mention, this runs irrespective of connection.

        // In future, when broadcast is implemented, this part is expected
        // to be isolated and the transmissions to be closed when there are
        // are no more peer connections.
        tokio::spawn(async move {
            let listener = Arc::new(tokio::net::UdpSocket::bind(LOCAL_RTP_ADDR).await.unwrap());
            let listener_1 = Arc::clone(&listener);

            let mut ffmpeg_process = tokio::process::Command::new("ffmpeg")
                .args(vec![
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-f",
                    "gdigrab",
                    "-i",
                    "desktop",
                    "-vf",
                    "scale=1280:720",
                    "-r",
                    "24",
                    "-pix_fmt",
                    "yuv420p",
                    "-c:v",
                    "libvpx",
                    "-b:v",
                    "2M",
                    "-f",
                    "rtp",
                    &format!("rtp://{LOCAL_RTP_ADDR}?pkt_size=1200"),
                ])
                .spawn()
                .unwrap();

            tokio::spawn(async move {
                let mut inbound_rtp_packet = vec![0u8; 1600];
                while let Ok((n, _)) = listener_1.recv_from(&mut inbound_rtp_packet).await {
                    if let Err(err) = track.write(&inbound_rtp_packet[..n]).await {
                        if webrtc::Error::ErrClosedPipe == err {
                            // Connection is closed.
                        } else {
                            // Some other error, yet to handle.
                        }
                        let _ = done_tx3.try_send(());
                        return;
                    }
                }
            });

            tokio::select! {
                _ = done_rx.recv() => {
                    eprintln!("Closing peer connection due to disconnection or failure signal.");
                    let _ = listener.close().await;
                    let _ = ffmpeg_process.start_kill();
                    let _ = peer.close().await;
                }
            };
        });

        anyhow::Ok(answer)
    }
}

pub struct CORS;

#[rocket::async_trait]
impl Fairing for CORS {
    fn info(&self) -> Info {
        Info {
            name: "Add CORS headers to responses",
            kind: Kind::Response,
        }
    }

    async fn on_response<'r>(&self, _request: &'r Request<'_>, response: &mut Response<'r>) {
        response.set_header(Header::new("Access-Control-Allow-Origin", "*"));
        response.set_header(Header::new(
            "Access-Control-Allow-Methods",
            "POST, GET, PATCH, OPTIONS",
        ));
        response.set_header(Header::new("Access-Control-Allow-Headers", "*"));
        response.set_header(Header::new("Access-Control-Allow-Credentials", "true"));
    }
}

#[post("/sdp", format = "json", data = "<session_description>")]
async fn sdp_endpoint(
    manager: &State<tokio::sync::Mutex<AetherWebRTCConnectionManager>>,
    session_description: rocket::serde::json::Json<RTCSessionDescription>,
) -> rocket::serde::json::Json<RTCSessionDescription> {
    rocket::serde::json::Json::from(
        manager
            .lock()
            .await
            .connect(session_description.into_inner())
            .await
            .unwrap(),
    )
}

#[options("/<_..>")]
fn all_options() {}

#[launch]
fn rocket() -> _ {
    let app = rocket::build();

    app.manage(tokio::sync::Mutex::new(
        AetherWebRTCConnectionManager::default().unwrap(),
    ))
    .mount("/", routes![sdp_endpoint, all_options])
    .attach(CORS)
}
