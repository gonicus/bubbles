# Bubbles - lightweight Linux working environments

**THIS PROJECT'S UX IS VERY MUCH IN AN UNPOLISHED STAGE**

**Quick**: Starts up in just a few seconds

**Integrated**: Wayland windows are managed on the host compositor

**Flexible**: Full access to mutable linux system

**Disposable**: Do not break your host; Break your bubble and discard it

**Isolated**: Strong KVM isolation boundary

**Atomic Desktop Friendly**: Works within e. g. Fedora Atomic desktops

**Rootless**: Does not require host root access

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

- Management of multiple VMs/Bubbles
- Distribution via flatpak
- MS Windows support
- More choices beyond Debian+Nix as guest system: e. g. Arch Linux
- Proper termination of passt+socat helpers

Imaginable opt-in Features:

- Option to share Nix store with other VMs/Bubbles
- Option to mount host directories
- Option to enable pulseaudio socket forwarding
- Option to promote `.desktop` files to host

### Limitations from upstream components

- EGL/GPU hardware acceleration (addressable using virtio native contexts?)
- For some Wayland applications, sommelier crashes

## Getting started

Right now, bubbles is distributed via a container outputting the required binaries into `$HOME/bubbles`.

Requirement: `podman`/`docker` for installation; `passt`

### Install

```
podman run -v "$HOME/bubbles:/output:Z" ghcr.io/gonicus/bubbles/bubbles:c3f4c775c99c7d946c1cccdafb477616c02e5fca
```

### Run

```
cd $HOME/bubbles
LD_LIBRARY_PATH=$HOME/bubbles/runtime_libs $HOME/bubbles/bubbles
```

1. Press image download button, await completion
2. Press VM creation button, restart bubbles
3. Start VM, await startup and initial setup
4. Press Terminal button
5. Enjoy mutable Debian+Nix Installation

### Cheat sheet

Enforcing Wayland:

- Chromium: `chromium --ozone-platform=wayland`
- Firefox: `WAYLAND_DISPLAY=wayland-0 firefox`
- VS Code:
    - `mkdir -p ~/.config/Code/User && echo '{"window.titleBarStyle": "custom"}' > ~/.config/Code/User/settings.json`
    - `code --ozone-platform=wayland`

Sound socket forwarding:

1. On host: `socat VSOCK-LISTEN:11112,fork UNIX-CONNECT:$XDG_RUNTIME_DIR/pulse/native`
2. On guest: `mkdir $XDG_RUNTIME_DIR/pulse && sudo chown user: $XDG_RUNTIME_DIR/pulse && socat UNIX-LISTEN:$XDG_RUNTIME_DIR/pulse/native,fork VSOCK-CONNECT:2:11112`

## Using the work in...

- crosvm + sommelier
- rust-gtk4
- passt
- distrobuilder
- ...
