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

See releases.

### Run

Start "Bubbles" via desktop, then:

1. Press image download button, await completion
   - This downloads a pre-built VM image (`disk.tar.gz`) published as a GitHub Release artifact, verifies its checksum, and extracts it locally.
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
$ /opt/home-manager-bootstrap init
$ /opt/home-manager-bootstrap switch
# Home Manager is initialized!
$ vim ~/.config/home-manager/home.nix # Add packages from nixpkgs
$ home-manager switch
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

## Using the work in...

- crosvm + sommelier
- Relm4
- rust-gtk4
- passt
- distrobuilder
- ...
