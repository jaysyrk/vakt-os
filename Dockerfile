FROM archlinux:latest

# Install base utilities
RUN pacman -Syu --noconfirm fastfetch tar

# Setup Vakt OS configuration
RUN mkdir -p /etc/fastfetch /opt/vakt/pkgs
COPY build-system/fastfetch/vakt_logo.txt /etc/fastfetch/
COPY build-system/fastfetch/config.jsonc /etc/fastfetch/

# Inject custom built binaries
COPY pkg-manager/target/release/zrpkg /usr/local/bin/
COPY tools/bin/vakt-audit /usr/local/bin/
COPY tools/bin/vakt-ids /usr/local/bin/
COPY vakt-verify/zig-out/bin/vakt-verify /usr/local/bin/

# Set up a badass custom prompt and auto-run fastfetch
RUN echo 'export PS1="\[\e[1;31m\][Vakt-OS]\[\e[0m\] \w # "' >> /root/.bashrc
RUN echo 'fastfetch' >> /root/.bashrc

WORKDIR /root
CMD ["/bin/bash"]
