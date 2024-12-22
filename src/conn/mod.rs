mod ffmpeg;
mod utils;

use std::sync::{atomic::AtomicBool, Arc};
use tokio::sync::RwLock;
use webrtc::api::media_engine::MIME_TYPE_H264;
use webrtc::api::API;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtcp::{
    payload_feedbacks::{
        full_intra_request::FullIntraRequest, picture_loss_indication::PictureLossIndication,
        receiver_estimated_maximum_bitrate::ReceiverEstimatedMaximumBitrate,
    },
    receiver_report::ReceiverReport,
};
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_local::TrackLocal;

pub struct AetherWebRTCConnectionManager {
    screen_track: RwLock<Option<Arc<TrackLocalStaticSample>>>,
    is_connected: Arc<AtomicBool>,
    rtc_configuration: RTCConfiguration,
    api: API,
}

impl AetherWebRTCConnectionManager {
    pub fn new(api: webrtc::api::API) -> Self {
        Self {
            screen_track: RwLock::new(None),
            is_connected: Arc::new(AtomicBool::new(false)),
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
        screen_track: Arc<TrackLocalStaticSample>,
    ) -> anyhow::Result<RTCPeerConnection> {
        let peer = self
            .api
            .new_peer_connection(self.rtc_configuration.clone())
            .await?;

        let rtp_sender = peer
            .add_track(Arc::clone(&screen_track) as Arc<dyn TrackLocal + Send + Sync>)
            .await?;

        tokio::spawn(async move {
            let mut rtcp_buf = vec![0u8; 1500];
            while let Ok((packets, _)) = rtp_sender.read(&mut rtcp_buf).await {
                for packet in packets {
                    if let Some(_) = packet.as_any().downcast_ref::<PictureLossIndication>() {
                        // Picture loss indication
                    } else if let Some(_) = packet.as_any().downcast_ref::<FullIntraRequest>() {
                        // Full intra request
                    } else if let Some(report) = packet.as_any().downcast_ref::<ReceiverReport>() {
                        if let Some(f) = report.reports.first() {
                            warn!("RTCP Report obtained: {:?}", f)
                        }
                    } else if let Some(bitrate) = packet
                        .as_any()
                        .downcast_ref::<ReceiverEstimatedMaximumBitrate>()
                    {
                        warn!("Estimated bitrate: {:.02}k", bitrate.bitrate / 1000_f32)
                    } else {
                        warn!("Unknown RTCP packet received.")
                    }
                }
            }
            anyhow::Result::<()>::Ok(())
        });

        Ok(peer)
    }

    async fn set_screen_source(
        &self,
        notifier: Option<Arc<tokio::sync::Notify>>,
        codec: &'static str,
    ) -> Option<Arc<tokio::sync::Notify>> {
        let mut re_ntfy = None;
        let mut loc_ntfy = None;

        let screen_track = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: codec.into(),
                ..Default::default()
            },
            "video".to_owned(),
            "aether-rtc-screen".to_owned(),
        ));

        if let Some(ntfy) = notifier {
            re_ntfy = Some(ntfy.clone());
            loc_ntfy = Some(ntfy.clone());
        }

        self.screen_track
            .write()
            .await
            .replace(screen_track.clone());

        let connection_state = self.is_connected.clone();

        tokio::spawn(async move {
            if let Some(ntfy) = loc_ntfy {
                ntfy.notified().await;
            }

            connection_state.store(true, std::sync::atomic::Ordering::Relaxed);

            let mut ffmpeg_process = std::process::Command::new("ffmpeg")
                .args(ffmpeg::get_ffmpeg_command())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();

            let reader = ffmpeg_process
                .stdout
                .take()
                .expect("Unable to access stdout, is it piped properly?");

            info!("Creating '{codec}' source for screen tracks.");

            if codec == MIME_TYPE_H264 {
                utils::h264_player_from(screen_track, connection_state, reader).await;
            } else {
                utils::ivf_player_from(screen_track, connection_state, reader).await;
            }
            info!("'{codec}' source exhausted.");

            ffmpeg_process.kill().unwrap_or_default();
        });

        re_ntfy
    }

    pub async fn connect(
        &mut self,
        offer: RTCSessionDescription,
    ) -> anyhow::Result<RTCSessionDescription> {
        let codec = utils::get_preferred_codec();

        let ntfy = Arc::new(tokio::sync::Notify::new());
        let mut re_ntfy = None;

        if self.screen_track.read().await.is_none() {
            re_ntfy = self.set_screen_source(Some(ntfy), codec).await
        }

        let datachannel_nfty = re_ntfy.clone();

        let screen_track = self
            .screen_track
            .read()
            .await
            .clone()
            .expect("Unable to load track after an expected load.");

        let peer = self.create_peer(screen_track).await?;

        let (done_tx, mut done_rx) = tokio::sync::mpsc::channel::<()>(1);
        let done_tx1 = done_tx.clone();

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

        peer.on_data_channel(Box::new(move |datachannel: Arc<RTCDataChannel>| {
            let mouse = mouse_rs::Mouse::new();
            // TODO
            // Fetch this through APIs.
            let (window_width, window_height) = (1920usize, 1080usize);

            let nfty = datachannel_nfty.clone();

            Box::pin(async move {
                datachannel.on_close(Box::new(move || Box::pin(async {})));
                datachannel.on_open(Box::new(move || Box::pin(async {})));

                let channel = datachannel.clone();

                datachannel.on_message(Box::new(move |msg: DataChannelMessage| {
                    match channel.label() {
                        "mouse_events" => {
                            if let Ok(message) =
                                serde_json::from_slice::<serde_json::Value>(&msg.data.to_vec())
                            {
                                let clicked_at = &message["payload"]["clicked_at"];

                                if let (Some(x), Some(y)) = (
                                    clicked_at["x_ratio"].as_f64(),
                                    clicked_at["y_ratio"].as_f64(),
                                ) {
                                    let previous_pos = mouse
                                        .get_position()
                                        .unwrap_or(mouse_rs::types::Point { x: 0, y: 0 });

                                    let _ = mouse.move_to(
                                        (window_width as f64 * x) as i32,
                                        (window_height as f64 * y) as i32,
                                    );
                                    let _ = mouse.click(&mouse_rs::types::keys::Keys::LEFT);
                                    let _ = mouse.move_to(previous_pos.x, previous_pos.y);
                                } else {
                                    error!("Unable to resolve click position from: {:?}", message);
                                }
                            } else {
                                error!(
                                    "Unexpected value in the mouse events data channel: {:?}",
                                    msg
                                );
                            }
                        }
                        "signalled_closure" => {
                            if let Some(ntfy) = &nfty {
                                ntfy.notify_one();
                            }
                        }
                        &_ => {}
                    }

                    Box::pin(async {})
                }));
            })
        }));

        peer.set_remote_description(offer).await?;

        let answer = peer.create_answer(None).await?;
        let mut gather_complete = peer.gathering_complete_promise().await;

        peer.set_local_description(answer.clone()).await?;
        let _ = gather_complete.recv().await;

        let conn = self.is_connected.clone();

        tokio::spawn(async move {
            tokio::select! {
                _ = done_rx.recv() => {
                    let _ = peer.close().await;
                    conn.store(false, std::sync::atomic::Ordering::Relaxed);
                }
            };
        });

        anyhow::Ok(answer)
    }
}
