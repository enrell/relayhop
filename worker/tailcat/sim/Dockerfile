FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends iproute2 iptables openssl python3 procps \
    && rm -rf /var/lib/apt/lists/*

COPY sim.sh /usr/local/bin/sim.sh
COPY derpmap.json /etc/sim/derpmap.json
RUN chmod +x /usr/local/bin/sim.sh

# Binaries (derper, probe) are bind-mounted at /opt/bin at run time;
# certs are generated inside the container on every run.
ENTRYPOINT ["/usr/local/bin/sim.sh"]
