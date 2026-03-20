#!/usr/bin/env bash
set -euo pipefail

# =============================================================================
# Download Firecracker kernel and rootfs images
# =============================================================================

ASSETS_DIR="${ASSETS_DIR:-/var/lib/mpp-hosting/assets}"
ARCH="$(uname -m)"

FC_RELEASE_URL="https://github.com/firecracker-microvm/firecracker/releases"
FC_VERSION="${FC_VERSION:-v1.11.0}"

mkdir -p "${ASSETS_DIR}"
cd "${ASSETS_DIR}"

# --- Download kernel ---
KERNEL_FILE="vmlinux-${FC_VERSION}-${ARCH}"
if [[ ! -f "vmlinux" ]]; then
    echo "Downloading kernel image..."

    # Use the CI artifacts bucket
    KERNEL_URL="https://s3.amazonaws.com/spec.ccfc.min/firecracker-ci/v1.11/${ARCH}/vmlinux-6.1.102"

    if curl -fsSL -o "${KERNEL_FILE}" "${KERNEL_URL}" 2>/dev/null; then
        ln -sf "${KERNEL_FILE}" vmlinux
        echo "Kernel downloaded: ${KERNEL_FILE}"
    else
        echo "WARNING: Could not download kernel from CI bucket."
        echo "Download manually from: ${FC_RELEASE_URL}"
        echo "Place as: ${ASSETS_DIR}/vmlinux"
    fi
else
    echo "Kernel already exists: vmlinux"
fi

# --- Download / create rootfs ---
if [[ ! -f "rootfs.ext4" ]]; then
    echo "Downloading Ubuntu rootfs..."

    ROOTFS_URL="https://s3.amazonaws.com/spec.ccfc.min/firecracker-ci/v1.11/${ARCH}/ubuntu-24.04.squashfs"

    if curl -fsSL -o "ubuntu.squashfs" "${ROOTFS_URL}" 2>/dev/null; then
        echo "Converting squashfs to ext4..."

        # Install squashfs-tools if needed
        which unsquashfs >/dev/null 2>&1 || apt-get install -y -qq squashfs-tools

        # Create ext4 image
        dd if=/dev/zero of=rootfs.ext4 bs=1M count=2048
        mkfs.ext4 -F rootfs.ext4

        MOUNT_DIR=$(mktemp -d)
        SQUASH_DIR=$(mktemp -d)

        mount rootfs.ext4 "${MOUNT_DIR}"
        unsquashfs -d "${SQUASH_DIR}/root" ubuntu.squashfs

        cp -a "${SQUASH_DIR}/root/"* "${MOUNT_DIR}/"

        # Configure networking inside the rootfs
        cat > "${MOUNT_DIR}/etc/resolv.conf" << 'DNS'
nameserver 8.8.8.8
nameserver 8.8.4.4
DNS

        # Enable SSH
        mkdir -p "${MOUNT_DIR}/root/.ssh"
        chmod 700 "${MOUNT_DIR}/root/.ssh"

        # Generate a host SSH key pair for provisioned VMs
        if [[ ! -f "${ASSETS_DIR}/vm_ssh_key" ]]; then
            ssh-keygen -t ed25519 -f "${ASSETS_DIR}/vm_ssh_key" -N "" -q
            echo "Generated VM SSH key pair"
        fi

        cp "${ASSETS_DIR}/vm_ssh_key.pub" "${MOUNT_DIR}/root/.ssh/authorized_keys"
        chmod 600 "${MOUNT_DIR}/root/.ssh/authorized_keys"

        umount "${MOUNT_DIR}"
        rm -rf "${MOUNT_DIR}" "${SQUASH_DIR}" ubuntu.squashfs

        echo "Rootfs created: rootfs.ext4 (2GB)"
    else
        echo "WARNING: Could not download rootfs."
        echo "You can create one manually. See Firecracker docs."
    fi
else
    echo "Rootfs already exists: rootfs.ext4"
fi

echo "Assets ready in: ${ASSETS_DIR}"
ls -lh "${ASSETS_DIR}/"
