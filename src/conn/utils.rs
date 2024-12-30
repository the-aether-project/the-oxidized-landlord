use std::sync::atomic::AtomicU16;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use webrtc::api::media_engine::{MIME_TYPE_H264, MIME_TYPE_VP8};
use webrtc::media::io::h264_reader::H264Reader;
use webrtc::media::io::ivf_reader::IVFReader;
use webrtc::media::io::ogg_reader::OggReader;

use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;

pub(crate) fn get_preferred_codec() -> &'static str {
    let linux_session_type = std::env::var("XDG_SESSION_TYPE");

    if let Ok(value) = linux_session_type {
        if value.as_str() == "wayland" {
            return MIME_TYPE_H264;
        }
    }

    return MIME_TYPE_VP8;
}

pub(crate) fn h264_player_from<T>(
    screen_track: Arc<TrackLocalStaticSample>,
    peer_count: Arc<RwLock<Vec<T>>>,
    reader: impl std::io::Read,
) -> impl std::future::Future<Output = ()> {
    async move {
        let mut h264_source = H264Reader::new(reader, 1_048_576);

        let mut ticker = tokio::time::interval(Duration::from_millis(33));

        while let Ok(nal) = h264_source.next_nal() {
            let sample = webrtc::media::Sample {
                data: nal.data.freeze(),
                duration: Duration::from_secs(1),
                ..Default::default()
            };

            if screen_track.write_sample(&sample).await.is_err() {
                break;
            }

            if peer_count.read().await.len() == 0 {
                break;
            }

            let _ = ticker.tick().await;
        }
    }
}

pub(crate) fn ivf_player_from<T>(
    screen_track: Arc<TrackLocalStaticSample>,
    peer_count: Arc<RwLock<Vec<T>>>,
    reader: impl std::io::Read,
) -> impl std::future::Future<Output = ()> {
    async move {
        let (mut ivf_source, header) = IVFReader::new(reader).unwrap();

        let duration = std::time::Duration::from_millis(
            ((1000 * header.timebase_numerator) / header.timebase_denominator) as u64,
        );

        while let Ok((frame, _)) = ivf_source.parse_next_frame() {
            let sample = webrtc::media::Sample {
                data: frame.freeze(),
                duration,
                ..Default::default()
            };

            if screen_track.write_sample(&sample).await.is_err() {
                break;
            }

            if peer_count.read().await.len() == 0 {
                break;
            }
        }
    }
}

pub(crate) fn opus_player_from(
    audio_track: Arc<TrackLocalStaticSample>,
    connection_state: Arc<AtomicU16>,
    reader: impl std::io::Read,
) -> impl std::future::Future<Output = ()> {
    async move {
        let (mut ogg, _) = OggReader::new(reader, false).unwrap();
        let mut ticker = tokio::time::interval(Duration::from_millis(20));
        let mut last_granule: u64 = 0;

        while let Ok((page_data, page_header)) = ogg.parse_next_page() {
            let sample_count = page_header.granule_position - last_granule;
            last_granule = page_header.granule_position;
            let sample_duration = Duration::from_millis(sample_count * 1000 / 48000);

            let sample = webrtc::media::Sample {
                data: page_data.freeze(),
                duration: sample_duration,
                ..Default::default()
            };

            if audio_track.write_sample(&sample).await.is_err() {
                break;
            }

            if connection_state.load(std::sync::atomic::Ordering::Relaxed) == 0 {
                break;
            }

            let _ = ticker.tick().await;
        }
    }
}
