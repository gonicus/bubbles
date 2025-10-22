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

## Using the work in...

- crosvm + sommelier
- rust-gtk4
- passt
- distrobuilder
- ...
