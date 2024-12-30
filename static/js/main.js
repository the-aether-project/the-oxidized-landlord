const videoPlayer = document.querySelector("video#player")
const audioPlayer = document.querySelector("audio#player")
const startButton = document.querySelector("button#start")
const closeButton = document.querySelector("button#close")

closeButton.disabled = true;

const portField = document.querySelector("input#local-port");
portField.value = portField.getAttribute("placeholder");

const ICE_SERVERS = [
    {
        "urls": [
            "stun:stun.l.google.com:19302"
        ]
    }
]

function openConnection() {

    var config = {
        iceServers: ICE_SERVERS,
        bundlePolicy: "max-bundle",
    }

    pc = new RTCPeerConnection(config);

    pc.addTransceiver('video', { direction: 'recvonly' });
    pc.addTransceiver('audio', { direction: 'recvonly' });

    var dataChannel = pc.createDataChannel("mouse_events");
    var signalledClosure = pc.createDataChannel("signalled_closure");

    const clickHandler = (event) => {

        const rect = videoPlayer.getBoundingClientRect();
        const x_ratio = (event.clientX - rect.left) / rect.width;
        const y_ratio = (event.clientY - rect.top) / rect.height;

        const click_payload = { x_ratio, y_ratio };

        dataChannel.send(
            JSON.stringify({
                type: "mouse",
                payload: { "clicked_at": click_payload },
            })
        );
    }

    dataChannel.onopen = () => {
        videoPlayer.addEventListener("click", clickHandler);
    }
    dataChannel.onclose = () => {
        videoPlayer.removeEventListener("click", clickHandler);
    }

    closeButton.addEventListener("click", () => {
        signalledClosure.send(JSON.stringify({
            type: "closure",
        }));
    });

    pc.addEventListener('track', (evt) => {
        if (evt.track.kind == 'video') {
            videoPlayer.srcObject = evt.streams[0];
            videoPlayer.onloadedmetadata = () => {
                videoPlayer.style.height = "60vh";
                videoPlayer.style.aspectRatio = `${videoPlayer.videoWidth}/${videoPlayer.videoHeight}`;
                videoPlayer.controls = false;
            }

            videoPlayer.play();
        }

        if (evt.track.kind == 'audio') {
            console.log("Audio track received")
            audioPlayer.srcObject = evt.streams[0];
            audioPlayer.play();
        }

        startButton.disabled = true;
        closeButton.disabled = false;
    });

    pc.createOffer().then((offer) => {
        return pc.setLocalDescription(offer);
    }).then(() => {
        return new Promise((resolve) => {
            if (pc.iceGatheringState === 'complete') {
                resolve();
            } else {
                const checkState = () => {
                    if (pc.iceGatheringState === 'complete') {
                        pc.removeEventListener('icegatheringstatechange', checkState);
                        resolve();
                    }
                };
                pc.addEventListener('icegatheringstatechange', checkState);
            }
        });
    }).then(() => {
        var offer = pc.localDescription;

        fetch(
            `/sdp`,
            {
                method: 'POST',
                body: JSON.stringify({
                    'sdp': offer.sdp,
                    'type': offer.type,
                }),
                headers: {
                    'Content-Type': 'application/json'
                }
            }
        ).then((response) => {
            if (response.ok) {
                return response.json();
            } else {
                throw new Error("Failed to send offer to the server.")
            }
        }).then((data) => {
            pc.setRemoteDescription(data);
        }).catch((e) => {
            alert(e);
        }
        )

    }).catch((e) => {
        alert(e);
    });


    let closeHandler = () => {
        pc.close();

        videoPlayer.srcObject.getTracks().forEach(track => track.stop());
        videoPlayer.srcObject = null;
        closeButton.removeEventListener("click", closeHandler)
        closeButton.disabled = true;
        startButton.disabled = false;
    };

    closeButton.addEventListener("click", closeHandler)
}


startButton.addEventListener("click", openConnection)
