# RelayHop network-simulation image: the real application plus a private DERP
# server and a set of network namespaces (carrier NATs, subscribers, core).
#
# Build from the repository root (see sim/run.sh):
#   docker build -f sim/Dockerfile -t relayhop-sim .
#
# Binaries are bind-mounted at /opt/bin at run time (nothing is baked in):
#   relayhop (Rust CLI), relayhop-tailcat (Go worker), derper (sim DERP server).
#
# archlinux:base is used because the host build links against the host glibc;
# the image tracks a rolling glibc that is >= the host's. A binary built on an
# older glibc also runs here.
FROM archlinux:base

RUN pacman -Syu --noconfirm --needed \
        iproute2 iptables openssl python3 procps \
        gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad gst-plugins-ugly gst-libav \
    && pacman -Scc --noconfirm

COPY sim/topo.sh sim/e2e.sh sim/assert.py /usr/local/bin/
COPY worker/tailcat/sim/derpmap.json /etc/sim/derpmap.json
RUN chmod +x /usr/local/bin/topo.sh /usr/local/bin/e2e.sh /usr/local/bin/assert.py

WORKDIR /sim
ENTRYPOINT ["/usr/local/bin/e2e.sh"]
