#!/bin/bash
# Create a Debian aarch64 rootfs with glibc and XFCE on an ext4 disk image
set -e

ROOTFS_IMG=target/rootfs.img
ROOTFS_SIZE=2048  # MiB

echo "Creating ${ROOTFS_SIZE}M rootfs using Docker..."

docker run --rm --privileged --platform linux/arm64 \
    -v "$(pwd)/target:/target" \
    debian:bookworm bash -c "
    set -e
    apt-get update -qq
    apt-get install -y -qq debootstrap e2fsprogs > /dev/null 2>&1

    # Create ext4 image
    dd if=/dev/zero of=/target/rootfs.img bs=1M count=${ROOTFS_SIZE} 2>/dev/null
    mkfs.ext4 -q -F /target/rootfs.img

    # Mount and populate
    mkdir -p /mnt/rootfs
    mount -o loop /target/rootfs.img /mnt/rootfs

    # Bootstrap minimal Debian
    debootstrap --arch=arm64 --variant=minbase \
        --include=bash,coreutils,libc6,libgcc-s1,libstdc++6,procps,sed,grep,findutils \
        bookworm /mnt/rootfs http://deb.debian.org/debian

    # Install XFCE and Wayland
    chroot /mnt/rootfs bash -c '
        apt-get update -qq
        DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
            xfce4 xfce4-terminal weston xwayland \
            dbus-x11 fonts-dejavu-core \
            libwayland-client0 libwayland-server0 \
            2>/dev/null || true
    '

    # Configure system
    echo 'root::0:0:root:/root:/bin/bash' > /mnt/rootfs/etc/passwd
    echo 'root:x:0:' > /mnt/rootfs/etc/group
    echo 'cyllor' > /mnt/rootfs/etc/hostname

    # Create init script
    cat > /mnt/rootfs/sbin/init << 'INITEOF'
#!/bin/bash
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export HOME=/root
export TERM=linux
export XDG_RUNTIME_DIR=/run/user/0
mkdir -p /run/user/0
echo 'Cyllor OS init'

# Start XFCE via Weston
if [ -x /usr/bin/weston ]; then
    weston --backend=drm-backend.so &
    sleep 2
    DISPLAY=:0 startxfce4 &
fi

exec /bin/bash
INITEOF
    chmod +x /mnt/rootfs/sbin/init

    umount /mnt/rootfs
    echo 'Rootfs created successfully!'
"

echo "Done: ${ROOTFS_IMG} ($(du -h ${ROOTFS_IMG} | cut -f1))"
