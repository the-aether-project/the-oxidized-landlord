mod ffmpeg;

use std::sync::Arc;
use webrtc::api::media_engine::MIME_TYPE_VP8;
use webrtc::api::API;
use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::media::io::ivf_reader::IVFReader;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_local::TrackLocal;

use tokio::sync::RwLock;

pub struct AetherWebRTCConnectionManager {
    screen_tracks: Arc<RwLock<Vec<Arc<TrackLocalStaticSample>>>>,
    rtc_configuration: RTCConfiguration,
    api: API,
}

impl AetherWebRTCConnectionManager {
    pub fn new(api: webrtc::api::API) -> Self {
        Self {
            screen_tracks: RwLock::new(vec![]).into(),
            rtc_configuration: RTCConfiguration {
                ice_servers: vec![RTCIceServer {
                    urls: vec!["stun:stun.l.google.com:19302".to_owned()],
                    ..Default::default()
                }],
                ..Default::default()
            },
            api,
        }
    }

    async fn create_peer(
        &mut self,
        screen_track: TrackLocalStaticSample,
    ) -> anyhow::Result<(RTCPeerConnection, usize)> {
        let screen_track = Arc::new(screen_track);

        let peer = self
            .api
            .new_peer_connection(self.rtc_configuration.clone())
            .await?;

        let rtp_sender = peer
            .add_track(Arc::clone(&screen_track) as Arc<dyn TrackLocal + Send + Sync>)
            .await?;

        tokio::spawn(async move {
            let mut rtcp_buf = vec![0u8; 1500];
            while let Ok((_, _)) = rtp_sender.read(&mut rtcp_buf).await {}
            anyhow::Result::<()>::Ok(())
        });

        self.screen_tracks.write().await.push(screen_track);

        Ok((peer, self.screen_tracks.read().await.len() - 1))
    }

    async fn set_screen_source(
        &self,
        notifier: Option<Arc<tokio::sync::Notify>>,
    ) -> Option<Arc<tokio::sync::Notify>> {
        let mut re_ntfy = None;
        let mut loc_ntfy = None;

        if let Some(ntfy) = notifier {
            re_ntfy = Some(ntfy.clone());
            loc_ntfy = Some(ntfy.clone());
        }

        let screen_tracks = self.screen_tracks.clone();

        tokio::spawn(async move {
            if let Some(ntfy) = loc_ntfy {
                ntfy.notified().await;
            }

            let mut ffmpeg_process = std::process::Command::new("ffmpeg")
                .args(ffmpeg::get_ffmpeg_command())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();

            let stdout = ffmpeg_process.stdout.take().unwrap();
            let (mut ivf_source, header) = IVFReader::new(stdout).unwrap();

            let duration = std::time::Duration::from_millis(
                ((1000 * header.timebase_numerator) / header.timebase_denominator) as u64,
            );

            let mut ticker = tokio::time::interval(duration);

            'outer: while let Ok((frame, _)) = ivf_source.parse_next_frame() {
                let _ = ticker.tick().await;

                let sample = webrtc::media::Sample {
                    data: frame.freeze(),
                    duration,
                    ..Default::default()
                };

                for track in screen_tracks.read().await.iter() {
                    if track.write_sample(&sample).await.is_err() {
                        break 'outer;
                    }
                }

                if screen_tracks.read().await.is_empty() {
                    break;
                }
            }

            ffmpeg_process.kill().unwrap_or_default();
        });

        re_ntfy
    }

    pub async fn connect(
        &mut self,
        offer: RTCSessionDescription,
    ) -> anyhow::Result<RTCSessionDescription> {
        let screen_track = TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: MIME_TYPE_VP8.to_owned(),
                ..Default::default()
            },
            "video".to_owned(),
            "aether-rtc-screen".to_owned(),
        );

        let (peer, track_index) = self.create_peer(screen_track).await?;

        let (done_tx, mut done_rx) = tokio::sync::mpsc::channel::<()>(1);
        let done_tx1 = done_tx.clone();

        let ntfy = Arc::new(tokio::sync::Notify::new());
        let mut re_ntfy = None;

        if self.screen_tracks.read().await.len() == 1 {
            re_ntfy = self.set_screen_source(Some(ntfy)).await
        }

        peer.on_ice_connection_state_change(Box::new(
            move |connection_state: RTCIceConnectionState| {
                match connection_state {
                    RTCIceConnectionState::Failed
                    | RTCIceConnectionState::Disconnected
                    | RTCIceConnectionState::Closed => {
                        let _ = done_tx1.try_send(());
                    }
                    RTCIceConnectionState::Connected => {
                        if let Some(ntfy) = &re_ntfy {
                            ntfy.notify_one();
                        };
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

        peer.set_local_description(answer.clone()).await?;
        let _ = gather_complete.recv().await;

        let tracks = self.screen_tracks.clone();

        tokio::spawn(async move {
            tokio::select! {
                _ = done_rx.recv() => {
                    let _ = peer.close().await;
                    tracks.write().await.remove(track_index);
                }
            };
        });

        anyhow::Ok(answer)
    }
}
