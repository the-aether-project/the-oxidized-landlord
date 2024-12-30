mod ffmpeg;
mod utils;
pub mod ws;

use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tokio::sync::{Notify, RwLock};
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

pub enum ConnectionStatus {
    ControlRelease(String),
    ControlTake(String),
    Connected(String),
    Disconnected(String),
}

pub struct AetherPeerConnection {
    pub peer_connection: Arc<RTCPeerConnection>,
    pub uuid: String,
    pub ntfy: Sender<()>,

    has_controls: bool,
    state_sender: Sender<ConnectionStatus>,
}

impl AetherPeerConnection {
    fn new(
        peer_connection: Arc<RTCPeerConnection>,
        uuid: String,
        ntfy: Sender<()>,
        sender: Sender<ConnectionStatus>,
    ) -> Self {
        Self {
            peer_connection,
            uuid,
            ntfy,
            has_controls: false,
            state_sender: sender,
        }
    }

    async fn take_control(&mut self) -> anyhow::Result<()> {
        self.state_sender
            .send(ConnectionStatus::ControlTake(self.uuid.clone()))
            .await?;
        self.has_controls = true;
        Ok(())
    }

    async fn release_control(&mut self) -> anyhow::Result<()> {
        self.state_sender
            .send(ConnectionStatus::ControlRelease(self.uuid.clone()))
            .await?;
        self.has_controls = false;
        Ok(())
    }

    async fn disconnect(&mut self) -> anyhow::Result<()> {
        if self.has_controls {
            self.release_control().await?
        }

        self.state_sender
            .send(ConnectionStatus::Disconnected(self.uuid.clone()))
            .await?;

        self.ntfy.send(()).await?;
        Ok(())
    }

    async fn connect(&self) -> anyhow::Result<()> {
        self.state_sender
            .send(ConnectionStatus::Connected(self.uuid.clone()))
            .await?;
        Ok(())
    }
}

pub struct AetherWebRTCConnectionManager {
    screen_track: Arc<RwLock<Option<Arc<TrackLocalStaticSample>>>>,
    rtc_configuration: RTCConfiguration,
    api: API,

    state_watcher: Sender<ConnectionStatus>,

    peers: Arc<RwLock<Vec<Arc<RwLock<AetherPeerConnection>>>>>,
}

mod peer_utils {
    use super::AetherPeerConnection;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    pub(super) async fn fetch_peer_by_uuid(
        peers: &Arc<RwLock<Vec<Arc<RwLock<AetherPeerConnection>>>>>,
        uuid: String,
    ) -> Option<Arc<RwLock<AetherPeerConnection>>> {
        for peer in peers.read().await.clone().into_iter() {
            if peer.read().await.uuid == uuid {
                return Some(peer.clone());
            }
        }

        None
    }

    pub(super) async fn discard_peer_by_uuid(
        peers: &Arc<RwLock<Vec<Arc<RwLock<AetherPeerConnection>>>>>,
        uuid: String,
    ) {
        for (n, peer) in peers.read().await.clone().into_iter().enumerate() {
            if peer.read().await.uuid == uuid {
                peers.write().await.remove(n);
            }
        }
    }

    pub(super) async fn fetch_peer_in_control(
        peers: &Arc<RwLock<Vec<Arc<RwLock<AetherPeerConnection>>>>>,
    ) -> Option<Arc<RwLock<AetherPeerConnection>>> {
        for peer in peers.read().await.clone().into_iter() {
            if peer.read().await.has_controls {
                return Some(peer.clone());
            }
        }

        None
    }
}

impl AetherWebRTCConnectionManager {
    pub fn new(api: webrtc::api::API, state_watcher: Sender<ConnectionStatus>) -> Self {
        Self {
            screen_track: RwLock::new(None).into(),
            rtc_configuration: RTCConfiguration {
                ice_servers: vec![RTCIceServer {
                    urls: vec!["stun:stun.l.google.com:19302".to_owned()],
                    ..Default::default()
                }],
                ..Default::default()
            },
            state_watcher: state_watcher,
            api,
            peers: RwLock::new(vec![]).into(),
        }
    }

    async fn change_control_to(&self, uuid: String) {
        for peer in self.peers.write().await.iter() {
            let mut peer_w = peer.write().await;
            if peer_w.uuid == uuid {
                let _ = peer_w.take_control().await;
            } else {
                let _ = peer_w.release_control().await;
            }
        }
    }

    async fn disconnect_peer(&self, uuid: String) -> anyhow::Result<()> {
        for peer in self.peers.write().await.iter() {
            let mut peer_w = peer.write().await;
            if peer_w.uuid == uuid {
                let _ = peer_w.disconnect().await?;
            }
        }

        Ok(())
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
            .add_track(screen_track.clone() as Arc<dyn TrackLocal + Send + Sync>)
            .await?;

        tokio::spawn(async move {
            let mut rtcp_buf = vec![0u8; 1500];
            while let Ok((packets, _)) = rtp_sender.read(&mut rtcp_buf).await {
                for packet in packets {
                    if let Some(pil) = packet.as_any().downcast_ref::<PictureLossIndication>() {
                        warn!("PIL obtained: {:?}", pil)
                    } else if let Some(fir) = packet.as_any().downcast_ref::<FullIntraRequest>() {
                        warn!("FIR obtained: {:?}", fir)
                    } else if let Some(report) = packet.as_any().downcast_ref::<ReceiverReport>() {
                        if let Some(f) = report.reports.first() {
                            info!("RTCP Report obtained: {:?}", f)
                        }
                    } else if let Some(bitrate) = packet
                        .as_any()
                        .downcast_ref::<ReceiverEstimatedMaximumBitrate>()
                    {
                        info!("Estimated bitrate: {:.02}k", bitrate.bitrate / 1000_f32)
                    } else {
                        warn!("Unknown RTCP packet received.")
                    }
                }
            }
            anyhow::Result::<()>::Ok(())
        });

        Ok(peer)
    }

    async fn set_screen_source(&self, notifier: Arc<Notify>, codec: &'static str) {
        let screen_track = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: codec.into(),
                ..Default::default()
            },
            "video".to_owned(),
            "aether-rtc-screen".to_owned(),
        ));

        self.screen_track
            .write()
            .await
            .replace(screen_track.clone());

        let track_copy = self.screen_track.clone();

        let peers_copy = self.peers.clone();

        tokio::spawn(async move {
            notifier.notified().await;

            let mut ffmpeg_process = std::process::Command::new("ffmpeg")
                .args(ffmpeg::get_ffmpeg_command())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("Unable to open ffmpeg, is it even in PATH?");

            let reader = ffmpeg_process
                .stdout
                .take()
                .expect("Unable to access stdout, is it piped properly?");

            info!("Creating '{codec}' source for screen tracks.");

            if codec == MIME_TYPE_H264 {
                utils::h264_player_from(screen_track, peers_copy, reader).await;
            } else {
                utils::ivf_player_from(screen_track, peers_copy, reader).await;
            }
            info!("'{codec}' source exhausted.");

            let _ = track_copy.write().await.take();
            ffmpeg_process.kill().unwrap_or_default();
        });
    }

    pub async fn connect(
        &mut self,
        offer: RTCSessionDescription,
        uuid: String,
    ) -> anyhow::Result<RTCSessionDescription> {
        let codec = utils::get_preferred_codec();

        let ntfy = Arc::new(Notify::new());

        if self.screen_track.read().await.is_none() {
            self.set_screen_source(ntfy.clone(), codec).await
        }

        let screen_track = self
            .screen_track
            .read()
            .await
            .clone()
            .expect("Unable to load track after an expected load.");

        let peer = self.create_peer(screen_track).await?;

        let (done_tx, mut done_rx) = tokio::sync::mpsc::channel::<()>(1);

        let associated_peer = Arc::new(RwLock::new(AetherPeerConnection::new(
            peer.into(),
            uuid,
            done_tx.clone(),
            self.state_watcher.clone(),
        )));

        let ice_nfty = ntfy.clone();
        let ice_done_tx = done_tx.clone();

        associated_peer
            .read()
            .await
            .peer_connection
            .on_ice_connection_state_change(Box::new(
                move |connection_state: RTCIceConnectionState| {
                    match connection_state {
                        RTCIceConnectionState::Failed
                        | RTCIceConnectionState::Disconnected
                        | RTCIceConnectionState::Closed => {
                            let _ = ice_done_tx.try_send(());
                        }
                        RTCIceConnectionState::Connected => {
                            ice_nfty.notify_waiters();
                        }
                        _ => {}
                    }

                    Box::pin(async {})
                },
            ));

        let auxilliary_peer_read = associated_peer.read().await;

        auxilliary_peer_read
            .peer_connection
            .on_peer_connection_state_change(Box::new(
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

        self.peers.write().await.push(associated_peer.clone());

        let peer_list_copy = self.peers.clone();
        let inner_peer = associated_peer.clone();

        let channel_ntfy = ntfy.clone();

        auxilliary_peer_read
            .peer_connection
            .on_data_channel(Box::new(move |datachannel: Arc<RTCDataChannel>| {
                let inner_peer = inner_peer.clone();
                let peer_list_copy = peer_list_copy.clone();

                let channel_ntfy = channel_ntfy.clone();

                Box::pin(async move {
                    datachannel.on_close(Box::new(move || Box::pin(async {})));
                    datachannel.on_open(Box::new(move || Box::pin(async {})));

                    let channel = datachannel.clone();
                    let channel_ntfy = channel_ntfy.clone();

                    datachannel.on_message(Box::new(move |msg: DataChannelMessage| {
                        let mut expected_mouse_ctrl = None;

                        match channel.label() {
                            "mouse_events" => {
                                if let Ok(message) =
                                    serde_json::from_slice::<serde_json::Value>(&msg.data.to_vec())
                                {
                                    let (window_width, window_height) = (1920usize, 1080usize);

                                    let clicked_at = &message["payload"]["clicked_at"];

                                    if let (Some(x), Some(y)) = (
                                        clicked_at["x_ratio"].as_f64(),
                                        clicked_at["y_ratio"].as_f64(),
                                    ) {
                                        expected_mouse_ctrl.replace((
                                            (window_width as f64 * x) as i32,
                                            (window_height as f64 * y) as i32,
                                        ));
                                    } else {
                                        error!(
                                            "Unable to resolve click position from: {:?}",
                                            message
                                        );
                                    }
                                } else {
                                    error!(
                                        "Unexpected value in the mouse events data channel: {:?}",
                                        msg
                                    );
                                }
                            }
                            "signalled_closure" => {
                                channel_ntfy.notify_waiters();
                            }
                            &_ => {}
                        };

                        let inner_peer = inner_peer.clone();
                        let peer_list_copy = peer_list_copy.clone();

                        Box::pin(async move {
                            if let Some((x, y)) = expected_mouse_ctrl {
                                if let Some(ctrl) =
                                    peer_utils::fetch_peer_in_control(&peer_list_copy).await
                                {
                                    if ctrl.read().await.uuid == inner_peer.read().await.uuid {
                                        let mouse = mouse_rs::Mouse::new();
                                        let _ = mouse.move_to(x, y);
                                        let _ = mouse.click(&mouse_rs::types::keys::Keys::LEFT);
                                    }
                                } else {
                                    let _ = inner_peer.write().await.take_control().await;
                                    let mouse = mouse_rs::Mouse::new();
                                    let _ = mouse.move_to(x, y);
                                    let _ = mouse.click(&mouse_rs::types::keys::Keys::LEFT);
                                }
                            }
                        })
                    }));
                })
            }));

        auxilliary_peer_read
            .peer_connection
            .set_remote_description(offer)
            .await?;

        let answer = auxilliary_peer_read
            .peer_connection
            .create_answer(None)
            .await?;
        let mut gather_complete = auxilliary_peer_read
            .peer_connection
            .gathering_complete_promise()
            .await;

        auxilliary_peer_read
            .peer_connection
            .set_local_description(answer.clone())
            .await?;
        let _ = gather_complete.recv().await;

        auxilliary_peer_read.connect().await?;

        if peer_utils::fetch_peer_in_control(&self.peers)
            .await
            .is_none()
        {
            let _ = associated_peer.write().await.take_control().await;
        }

        let watching_peer = associated_peer.clone();
        let peer_list = self.peers.clone();

        tokio::spawn(async move {
            tokio::select! {
                _ = done_rx.recv() => {
                    let _ = watching_peer.read().await.peer_connection.close().await;
                    let mut mut_associated_peer = watching_peer.write().await;

                    if mut_associated_peer.has_controls {
                        let _ = mut_associated_peer.release_control().await;
                    }
                    let _ = mut_associated_peer.disconnect().await;
                    peer_utils::discard_peer_by_uuid(&peer_list, mut_associated_peer.uuid.clone()).await;
                }
            };
        });

        anyhow::Ok(answer)
    }
}
