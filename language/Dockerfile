FROM rust:1.61.0-bullseye

ARG USER_ID=1000
COPY ci/user.sh .
RUN ./user.sh $USER_ID

ARG RUDDER_VER=latest

COPY ci/rust.sh .
RUN ./rust.sh

ENV RUSTC_WRAPPER="sccache"

RUN apt-get update && apt-get install -y python3 perl pandoc asciidoctor rsync

# we need cf-promises
# the easiest way is to install an agent
RUN wget https://repository.rudder.io/tools/rudder-setup
# ignore postinst failures
RUN sh rudder-setup setup-agent $RUDDER_VER || true
