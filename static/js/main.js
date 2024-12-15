const videoPlayer = document.querySelector("video#player")
const startButton = document.querySelector("button#start")
const closeButton = document.querySelector("button#close")

closeButton.disabled = true;

const portField = document.querySelector("input#local-port");
portField.value = "8000";

const ICE_SERVERS = [
    {
        "urls": [
            "stun:global.stun.twilio.com:3478"
        ],
        "username": "2e449eecea3a37550670e978ce498c63e60d2bf188713584f5bcef6f0f1364e9",
        "credential": "6YEcCbazWfwnbgHnkQQWlaV2cfSL/VHUrGXriqPH39k=",
        "credential_type": "Password"
    },
    {
        "urls": [
            "turn:global.turn.twilio.com:3478?transport=udp"
        ],
        "username": "2e449eecea3a37550670e978ce498c63e60d2bf188713584f5bcef6f0f1364e9",
        "credential": "6YEcCbazWfwnbgHnkQQWlaV2cfSL/VHUrGXriqPH39k=",
        "credential_type": "Password"
    },
    {
        "urls": [
            "turn:global.turn.twilio.com:3478?transport=tcp"
        ],
        "username": "2e449eecea3a37550670e978ce498c63e60d2bf188713584f5bcef6f0f1364e9",
        "credential": "6YEcCbazWfwnbgHnkQQWlaV2cfSL/VHUrGXriqPH39k=",
        "credential_type": "Password"
    },
    {
        "urls": [
            "turn:global.turn.twilio.com:443?transport=tcp"
        ],
        "username": "2e449eecea3a37550670e978ce498c63e60d2bf188713584f5bcef6f0f1364e9",
        "credential": "6YEcCbazWfwnbgHnkQQWlaV2cfSL/VHUrGXriqPH39k=",
        "credential_type": "Password"
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
            `http://127.0.0.1:${portField.value}/sdp`,
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
