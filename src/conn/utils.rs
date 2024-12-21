use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
use webrtc::api::media_engine::{MIME_TYPE_H264, MIME_TYPE_VP8};
use webrtc::media::io::h264_reader::H264Reader;
use webrtc::media::io::ivf_reader::IVFReader;
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

pub(crate) fn h264_player_from(
    screen_track: Arc<TrackLocalStaticSample>,
    connection_state: Arc<AtomicBool>,
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

            if !connection_state.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }

            let _ = ticker.tick().await;
        }
    }
}

pub(crate) fn ivf_player_from(
    screen_track: Arc<TrackLocalStaticSample>,
    connection_state: Arc<AtomicBool>,
    reader: impl std::io::Read,
) -> impl std::future::Future<Output = ()> {
    async move {
        let (mut ivf_source, header) = IVFReader::new(reader).unwrap();

        let duration = std::time::Duration::from_millis(
            ((1000 * header.timebase_numerator) / header.timebase_denominator) as u64,
        );

        let mut start_time = std::time::Instant::now();

        while let Ok((frame, _)) = ivf_source.parse_next_frame() {
            start_time += duration;

            let sample = webrtc::media::Sample {
                data: frame.freeze(),
                duration,
                ..Default::default()
            };

            if screen_track.write_sample(&sample).await.is_err() {
                break;
            }

            if !connection_state.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }

            let delta = std::time::Instant::now().duration_since(start_time);

            if delta > duration * 2 {
                warn!("Running behind by {}ms.", delta.as_millis());
            }
        }
    }
}
