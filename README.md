# Bubbles - lightweight Linux working environments

**Quick**: Starts up in just a few seconds

**Disposable**: Do not break your host; Break your bubble and discard it

**Isolated**: Strong KVM isolation boundary

**Immutable**: Includes Nix to enable version-controlled, reproducible work environments

**Mutable**: If Nix is too strict, fall back on Debian's apt or install any other package manager

**Atomic Desktop Friendly**: Works within e. g. Fedora Atomic desktops

**Rootless**: Does not require host root access

**Integrated**: Wayland windows are managed on the host compositor

## Getting started

Right now, bubbles is distributed via a container outputting the required binaries into `$HOME/bubbles`.

Requirements:
- `podman`/`docker` for installation
- `passt`
- `qemu-img`
- `curl`

Loose Recommendation:
- `btrfs` as backing filesystem (seems to optimize for disk image deduplication under the hood)

### Install

```
mkdir $HOME/bubbles
# May be different for non-SELinux systems: skip ":Z"
# May be different for docker: You may need to chown files afterwards
podman run -v "$HOME/bubbles:/output:Z" ghcr.io/gonicus/bubbles/bubbles:841f165307e5d15b789cd8fc1aab40b7ecef6f3e
```

### Run

```
cd $HOME/bubbles
LD_LIBRARY_PATH=$HOME/bubbles/runtime_libs $HOME/bubbles/bubbles
```

1. Press image download button, await completion
2. Press VM creation button, enter name, confirm
3. Start VM, await startup and initial setup
4. Press Terminal button
5. Enjoy mutable Debian+Nix Installation
6. (Optional, yet recommended: Setup Nix home-manager, see "Cheat Sheet")

The installed system is a Debian Trixie with preinstalled...
- Gnome Console (kgx)
- Nix 
- sommelier
- starship (configured for nerdfonts)
- bubbles-agent (simple agent for serving needs of the UI)

On first boot, it will fetch a nerdfont.

### Cheat sheet

#### Install home-manager (recommended, it's worth it)

```
$ sudo nix-channel --update
$ nix-shell -p home-manager
$ home-manager init
$ vim /home/user/.config/home-manager/home.nix # Add packages from nixpkgs
$ home-manager switch # Ensure that /home/user/.nix-profile/bin is in PATH afterwards
```

#### Change default terminal

- `sudo update-alternatives --config x-terminal-emulator`

#### Enforcing Wayland

- Chromium: `chromium --ozone-platform=wayland`
- Firefox: `WAYLAND_DISPLAY=wayland-0 firefox`
- VS Code:
    - `mkdir -p ~/.config/Code/User && echo '{"window.titleBarStyle": "custom"}' > ~/.config/Code/User/settings.json`
    - `code --ozone-platform=wayland`

#### Sound socket forwarding

1. On host: `socat VSOCK-LISTEN:11112,fork UNIX-CONNECT:$XDG_RUNTIME_DIR/pulse/native`
2. On guest: `mkdir $XDG_RUNTIME_DIR/pulse && sudo chown user: $XDG_RUNTIME_DIR/pulse && socat UNIX-LISTEN:$XDG_RUNTIME_DIR/pulse/native,fork VSOCK-CONNECT:2:11112`

## Comparisons

<details>
<summary>Compared to distroboxes...</summary>

Pro Bubbles:
- allows straight-forward use of containers
- provides isolation

Contra Bubbles:
- not as host-integrated as distroboxes

</details>


<details>
<summary>Compared to devcontainers...</summary>

Pro Bubbles:
- allows straight-forward use of containers (hence also devcontainers)

Contra Bubbles:
- not part of devcontainer ecosystem

</details>

<details>
<summary>Compared to allround VM solutions like Gnome Boxes...</summary>

Pro Bubbles:
- does not require stepping through OS installers
- opinionated networking etc.
- allows Wayland integration

Contra Bubbles:
- does not support traditional VM handling use cases

</details>

## Current limitations

### TODO's in Bubbles

- Distribution via flatpak
- MS Windows support
- More choices beyond Debian+Nix as guest system: e. g. Arch Linux

Imaginable opt-in Features:

- Option to share Nix store with other VMs/Bubbles
- Option to mount host directories
- Option to enable pulseaudio socket forwarding
- Option to promote `.desktop` files to host

### Limitations from upstream components

- EGL/GPU hardware acceleration not trivial
    - For AMD, addressable using virtio native contexts, WIP
- For some Wayland applications, sommelier crashes

## Using the work in...

- crosvm + sommelier
- Relm4
- rust-gtk4
- passt
- distrobuilder
- ...
