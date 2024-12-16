#[macro_use]
extern crate rocket;
use rocket::State;
use std::sync::Arc;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_H264};
use webrtc::api::{APIBuilder, API};
use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::media::io::h264_reader::H264Reader;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::TrackLocal;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use rocket::fairing::{Fairing, Info, Kind};
use rocket::http::Header;
use rocket::{Request, Response};


struct AetherWebRTCConnectionManager {
    screen_tracks: Arc<tokio::sync::Mutex<Vec<Arc<TrackLocalStaticSample>>>>,
    rtc_configuration: RTCConfiguration,
    api: API,
}

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
            screen_tracks: Arc::new(tokio::sync::Mutex::new(vec![])),
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
        let screen_track = Arc::new(TrackLocalStaticSample::new(
                RTCRtpCodecCapability {
                    mime_type: MIME_TYPE_H264.to_owned(),
                    ..Default::default()
                },
                "video".to_owned(),
                "aether-rtc-screen".to_owned(),
            ));

            
        self.screen_tracks.lock().await.push(Arc::clone(&screen_track));
        let track_index = self.screen_tracks.lock().await.len() - 1;
        


        let peer = Arc::new(
            self.api
                .new_peer_connection(self.rtc_configuration.clone())
                .await?,
        );

        let rtp_sender = peer
            .add_track(Arc::clone(&screen_track) as Arc<dyn TrackLocal + Send + Sync>)
            .await?;

        tokio::spawn(async move {
            let mut rtcp_buf = vec![0u8; 1500];
            while let Ok((_, _)) = rtp_sender.read(&mut rtcp_buf).await {}
            anyhow::Result::<()>::Ok(())
        });

        let (done_tx, mut done_rx) = tokio::sync::mpsc::channel::<()>(1);
        let done_tx1 = done_tx.clone();


        let notify = Arc::new(tokio::sync::Notify::new());
        let notify2 = Arc::clone(&notify);

        peer.on_ice_connection_state_change(Box::new(
            move |connection_state: RTCIceConnectionState| {
                match connection_state {
                    RTCIceConnectionState::Failed
                    | RTCIceConnectionState::Disconnected
                    | RTCIceConnectionState::Closed => {
                        let _ = done_tx1.try_send(());
                    }
                    RTCIceConnectionState::Connected => {
                        notify.notify_one();
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
                        let _ = done_tx.try_send(());
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

        let answer = peer.local_description().await.unwrap();

        let screen_tracks = Arc::clone(&self.screen_tracks);
        let screen_tracks_2 = Arc::clone(&self.screen_tracks);

        if self.screen_tracks.lock().await.len() == 1 {
            tokio::spawn(async move {
                notify2.notified().await;

                tokio::spawn(async move {
                    let ffmpeg_process = std::process::Command::new("ffmpeg")
                    .args(vec![
                        "-re",
                        "-device",
                        "/dev/dri/card1",
                        "-f",
                        "kmsgrab",
                        "-i",
                        "-",
                        "-vf",
                        "hwmap=derive_device=vaapi,scale_vaapi=w=1280:h=720:format=nv12",
                        "-c:v",
                        "h264_vaapi",
                        "-bsf:v",
                        "h264_mp4toannexb",
                        "-r",
                        "24",
                        "-b:v",
                        "1M",
                        "-f",
                        "h264",
                        "-"
                    ])
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .unwrap();

                    let stdout = ffmpeg_process.stdout.unwrap();
                    let mut h264_source = H264Reader::new(stdout, 1_048_576);

                    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(33));

                    while let Ok(nal) = h264_source.next_nal() {
                        let sample = webrtc::media::Sample {
                            data: nal.data.freeze(),
                            duration: std::time::Duration::from_secs(1),
                            ..Default::default()
                        };
                        
                        for track in screen_tracks.lock().await.iter() {
                            if track.write_sample(&sample).await.is_err() {
                                break
                            }
                        }

                        let _ = ticker.tick().await;

                        if screen_tracks.lock().await.is_empty() {
                            break;
                        }

                    }
                });

                
            });
        }

        tokio::spawn(async move {tokio::select! {
            _ = done_rx.recv() => {
                let _ = peer.close().await;
                screen_tracks_2.lock().await.remove(track_index);
            }
        };});

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
