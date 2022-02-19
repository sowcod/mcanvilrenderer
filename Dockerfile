FROM rust:1.58

WORKDIR /usr/src/myapp

RUN git clone https://github.com/sowcod/mcanvilrenderer.git \
    && cd mcanvilrenderer \
    && cargo build --release
