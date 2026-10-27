#!/usr/bin/env python3
"""The plan job of ci-runtime.yml: which boot tests a change needs, from the
files it touches (docs/testing.md, "In CI").

Every boot test carries tags (`t -t TAGS name ...` in user/tests/kernel.sh
and the ports' test.sh; a port's name on every test of its file, `pkg` on a
package's). A changed path selects tags: a port's name and the names of the
ports built on it, a module's name, `linux` for the Linux layer; the
kernel, the toolchain, the launcher, the scripts and the workflows select
everything, documentation nothing. `smoke` (the quick basics) is always
in. A push to master, the daily run, a dispatch with full_boot and a pull
request labelled `ci:full` run everything.

Each arch has a primary machine (uefi, aarch64 with GICv3, riscv64 with
the AIA) and variants (bios, bios with AMD's IOMMU, aarch64 with GICv2,
riscv64 with the PLIC). A run of
everything boots everything on every machine, as does a change to what
differs between an arch's machines (MACHINE_SPECIFIC: the IOMMUs, the
boot files, the launcher, the workflows). A pull request's change
otherwise boots on the primaries: everything on them and the smoke set on
the variants when it selects everything (the kernel, the toolchain), its
tags on them alone when it selects tags; a change under
kernel/src/arch/<arch> boots everything on that arch's machines, variants
too. A machine's selection runs in parallel boots: the long tests'
groups (`ostest`, `alpine`, `pkg`: each a boot of its own, whole, as its
tests build on each other) and one light boot with the rest. On aarch64
and riscv64, which run under TCG, the groups take two to three times what
the light boot does: a pull request boots the light tests there alone,
and the groups on x86 (under KVM), unless the change is one the groups
have caught arch-specific breakage of before (GROUP_PATHS: get-myos's
install and upgrade, the Linux layer, os-test) or is under
kernel/src/arch/<arch>; a run of everything boots them everywhere. The
Raspberry Pi 3 boots its smoke tests whenever an aarch64 machine boots.
Each boot gets a budget from what it runs, and says whether it needs the
Alpine disk (`alpine`, the `alpine-disk` tag: the boot job restores it
from the cache).

Writes GitHub outputs ($GITHUB_OUTPUT, stdout without it):
  boots=<JSON>     the boot job's matrix (`[]`: no boot)
  features=        `linux_compat` when a Linux layer test runs (the image
                   carries the layer), else empty
and a summary to $GITHUB_STEP_SUMMARY when set.

Usage: ci-select.py [--all] [--event NAME] [--full-label] [--base SHA [--head REF]]
                    [--changed PATH...]
"""
import json
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# The boot machines: id, the launcher's arch word, the arch, the variant
# (aarch64: the GIC version, `MYOS_AARCH64_GIC`; x86_64: the IOMMU,
# `MYOS_X86_IOMMU`; riscv64: the interrupt controllers, `MYOS_RISCV_INTC`),
# and whether it is the arch's primary machine, the one
# a pull request's selection boots on (`plan`). The variants differ from
# it in the firmware, the IOMMU or the interrupt controller: what a
# MACHINE_SPECIFIC path or a run of everything exercises. The AMD IOMMU
# machine and the riscv64 PLIC one boot the light tests only: the long
# groups exercise nothing the unit or the controller changes.
MACHINES = [
    ("bios", "", "x86_64", "", False),
    ("uefi", "uefi", "x86_64", "", True),
    ("bios-amd", "", "x86_64", "amd", False),
    ("aarch64", "aarch64", "aarch64", "", False),
    ("aarch64-gicv3", "aarch64", "aarch64", "3", True),
    ("riscv64", "riscv64", "riscv64", "", True),
    ("riscv64-plic", "riscv64", "riscv64", "plic", False),
]
# Paths whose change is every machine's, not the primaries' alone: what
# differs between an arch's machines (the IOMMUs, the boot protocol and
# the boot files, the launcher's QEMU per machine, the workflows).
MACHINE_SPECIFIC = re.compile(r"^(kernel/src/iommu/|kernel/src/limine_boot\.rs|src/|boot/|\.github/|scripts/ci-)")
# Paths whose change boots the long groups on aarch64 and riscv64 in a
# pull request too (`plan`): where the groups have found breakage of one
# arch's before (get-myos's install and upgrade: riscv64's lseek, brk heap
# and ESP; the Linux layer: musl's vfork on aarch64 and riscv64; os-test's
# semantics, with per-arch issues open). A port, a package or the kernel
# outside its arch directories gets them on x86 alone, and on every
# machine once merged.
GROUP_PATHS = re.compile(r"^(user/get-myos/|linux-compat/|packages/os-test/)")
# The Raspberry Pi 3 (QEMU raspi3b, `myos rpi3 test`, docs/raspberry-pi.md):
# no disk but the SD card it boots from (`sdmmc` checks it), no network or
# PCI; its USB (the dwc2 module) carries QEMU's hub with the stick, the
# mouse and the keyboard the tests plug in. One boot of the smoke tests it
# can run and the USB tests of its devices (`/dev/usb`'s serial adapter
# among them), whenever an aarch64 machine
# boots: the tests that need the scratch disk (`partitions`, `ext2_disk`,
# `fstab_mount`, `fileio`), the ones that need the network (`boot_rc`: the
# rc.local's dropbear; `heap_mini`: its ping and socket stages), the other
# pointers' (`ps2_mouse`, `virtio_mouse`,
# `xev_pointer`: a package's) and the USB tests of a root port of their
# own (`usb_hotplug`, `usb_hotplug_busy_reuse`, `usb_hub_replug`) stay out.
RPI3_ARGS = (
    "rpi3 test --tags smoke,usb_disk,mouse,usb_keyboard,usbdev"
    " --skip partitions,ext2_disk,fstab_mount,fileio,boot_rc,heap_mini,ps2_mouse,virtio_mouse,xev_pointer"
    " --budget 20"
)
# The long tests' groups, each a boot of its own: the group's name and the
# tags of its tests. `ostest` and `alpine` (the curated os-test list, the
# apk installs) build on each other; `rust` sets up the Rust toolchain in
# the guest and builds with it (the rust package's test: a download and
# minutes of rustc under emulation, too long for the pkg boot, which would
# install every package first); `pkg` is the packages' install from
# the mirror (the runner's `packages` test, six minutes on riscv64), the
# packages' and get-myos's tests on it and the `install` tests of the
# boot disk, which build on the install too. A test tagged with two
# groups' tags is the first group's; what is tagged none of them is the
# light boot's (a `heavy` test there, the heavy smoke, stays with the
# rest).
HEAVY_GROUPS = {"ostest": ("ostest",), "alpine": ("alpine",), "rust": ("rust",), "pkg": ("pkg", "install")}
# The budget of a boot in minutes, by what it runs; riscv64 (the slowest
# under TCG) gets half as much again. A budget is a guard against a hang,
# not a target, and a slow runner takes up to twice a good one's time:
# the aarch64 Alpine boot takes 9 to 10 minutes, the aarch64 os-test and
# pkg boots (every package's install first, python's library among them)
# 8 on a good runner and 14 on a slow one; on riscv64 the light boot takes
# 6, the os-test and pkg boots 11 and 12.
BUDGETS = {"light": 15, "smoke": 8, "ostest": 20, "alpine": 15, "rust": 30, "pkg": 20}


def group_of(tags):
    """The long group a test with `tags` belongs to, `None` for the light
    boot."""
    for g, gtags in HEAVY_GROUPS.items():
        if tags & set(gtags):
            return g
    return None
# A module's directory selects these tags (its own name otherwise).
MODULE_TAGS = {
    "console": {"console", "tty", "mouse"},
    "virtio_net": {"virtio_net", "netfs"},
    "virtio": {"blk", "virtio_net"},
    "virtq": {"blk", "virtio_net"},
    "virtio_blk": {"blk"},
    "nvme": {"blk"},
    "sdmmc": {"blk", "sdmmc", "sdio"},
    "rpi_firmware": {"rpi_firmware"},
    "virtio_scsi": {"blk"},
    "usb": {"usb"},
    "xhci": {"usb"},
    "ehci": {"usb"},
    "uhci": {"usb", "sound"},
    "ohci": {"usb", "sound"},
    "dwc2": {"usb"},
    "usb_hub": {"usb"},
    "usb_storage": {"usb"},
    "usb_hid": {"usb", "mouse"},
    "usb_audio": {"usb", "sound"},
    "usbdev": {"usb", "usbdev"},
    "virtio_sound": {"sound"},
    "pcidev": {"pcidev"},
    "pl061": {"irq"},
    "tap": {"tap", "netfs"},
    "hello": {"modules"},
}
# Modules whose change is the whole system's (the ABI, the platform).
MODULES_ALL = {"abi", "acpi", "pci_enum"}
# A port's directory selects its own tests and these (what it implements).
PORT_EXTRA_TAGS = {"netd": {"netfs"}, "mkfs.ext2": {"ext2"}, "mkfs.fat": {"fat"}, "get-myos": {"install", "pkg"}}
# Ports whose change is the whole system's.
PORTS_ALL = {"init", "tests", "newlib", "sysroot", "insmod", "mount", "umount"}
# Documentation: no boot.
DOCS = re.compile(r"(^|/)([^/]*\.md|LICENSE[^/]*|\.gitignore)$|^(docs|licenses)/")


class Everything:
    """The selection of every test."""


def read_env(path):
    """The `KEY=value` lines of a port.env (values may be quoted)."""
    env = {}
    with open(path, encoding="utf-8") as f:
        for line in f:
            m = re.match(r"^([A-Z_]+)=(.*)$", line.strip())
            if m:
                env[m.group(1)] = m.group(2).strip().strip('"').strip("'")
    return env


def load_ports():
    """Every port: name -> (dir, role, test file, deps, rdeps)."""
    ports = {}
    for base, role in [("ports", "image"), ("user", "image"), ("toolchain", "image"), ("packages", "package")]:
        root = os.path.join(ROOT, base)
        if not os.path.isdir(root):
            continue
        for d in sorted(os.listdir(root)):
            env_path = os.path.join(root, d, "port.env")
            if not os.path.isfile(env_path):
                continue
            env = read_env(env_path)
            name = env.get("PORT_NAME", d)
            ports[name] = {
                "dir": f"{base}/{d}",
                "role": role,
                "test": env.get("PORT_TEST", ""),
                "deps": env.get("PORT_DEPS", "").split(),
                "rdeps": env.get("PORT_RDEPS", "").split(),
            }
    return ports


TEST_LINE = re.compile(r"^\s*(?:t|t_last)\s+(?:-t\s+(\S+)\s+)?([A-Za-z0-9_]+)")


def load_tests(ports):
    """Every boot test: name -> set of tags (the runner's own `packages`
    install test included)."""
    tests = {"packages": {"net", "pkg"}}

    def scan(path, file_tags):
        with open(path, encoding="utf-8") as f:
            for line in f:
                m = TEST_LINE.match(line)
                if m:
                    tags = set(m.group(1).split(",")) if m.group(1) else set()
                    tests[m.group(2)] = tags | file_tags

    scan(os.path.join(ROOT, "user/tests/kernel.sh"), set())
    for name, p in ports.items():
        if p["test"]:
            file_tags = {name} | ({"pkg"} if p["role"] == "package" else set())
            scan(os.path.join(ROOT, p["dir"], p["test"]), file_tags)
    return tests


def dependents(ports, name):
    """The ports built on `name` (PORT_DEPS) or needing it on the running
    system (PORT_RDEPS), transitively."""
    out = set()
    todo = [name]
    while todo:
        n = todo.pop()
        for other, p in ports.items():
            if n in p["deps"] or n in p["rdeps"]:
                if other not in out:
                    out.add(other)
                    todo.append(other)
    return out


def classify(path, ports):
    """What a changed path selects: `None` (nothing), a set of tags, or
    `Everything` (with the arch it is confined to, else None)."""
    if DOCS.search(path):
        return None, None
    m = re.match(r"^kernel/src/arch/(x86|aarch64|riscv)", path)
    if m:
        return Everything, {"x86": "x86_64", "aarch64": "aarch64", "riscv": "riscv64"}[m.group(1)]
    m = re.match(r"^modules/([^/]+)/", path)
    if m:
        mod = m.group(1)
        if mod in MODULES_ALL:
            return Everything, None
        return MODULE_TAGS.get(mod, {mod}), None
    if path.startswith("linux-compat/"):
        return {"linux"}, None
    for name, p in ports.items():
        if path.startswith(p["dir"] + "/"):
            if name in PORTS_ALL:
                return Everything, None
            tags = {name} | dependents(ports, name) | PORT_EXTRA_TAGS.get(name, set())
            return tags, None
    # The kernel, the toolchain, the launcher, the build, the scripts, the
    # workflows, the policy, anything unknown: everything.
    return Everything, None


def plan(changed, everything, ports, tests):
    """The selection per machine (the tests it boots, and the tags they
    were selected by: `None` when it boots everything), whether the Linux
    layer must be in the image, and the tags.

    A run of everything, and a change to what differs between an arch's
    machines (MACHINE_SPECIFIC), boot everything on every machine. A
    change under kernel/src/arch/<arch> boots everything on that arch's
    machines. Otherwise only the primary machines boot: everything when
    the change selects everything (the variants the smoke set then), the
    change's tags when it selects tags (the variants nothing). The long
    groups boot on aarch64 and riscv64 only in a run of everything, for a
    GROUP_PATHS change or for a change under kernel/src/arch/<arch>: a
    pull request's other changes boot the light tests there (`light`:
    True in the selection), the groups on x86."""
    tags = {"smoke"}
    all_on = set()  # the arches that run everything, variants too
    whole = False  # the change selects everything, not for one arch
    every_machine = everything
    groups_everywhere = everything
    if not everything:
        if not changed:
            return {}, False, None
        kinds = [classify(path, ports) for path in changed]
        if all(what is None for what, _ in kinds):
            return {}, False, None
        for path, (what, arch) in zip(changed, kinds):
            if what is None:
                continue
            if what is Everything:
                if arch is None:
                    whole = True
                else:
                    all_on.add(arch)
            else:
                tags |= what
            if MACHINE_SPECIFIC.match(path):
                every_machine = True
            if GROUP_PATHS.match(path):
                groups_everywhere = True
    tags_arg = ",".join(sorted(tags))
    by_tags = {n for n, t in tests.items() if t & tags}
    smoke = {n for n, t in tests.items() if "smoke" in t}
    # The runner installs the packages before a `pkg` test whatever the
    # selection says: that boot needs the mirror and the time.
    for names in (by_tags, smoke):
        if any("pkg" in tests[n] for n in names):
            names.add("packages")
    selection = {}
    linux = False
    for mid, _, arch, _, primary in MACHINES:
        if every_machine or arch in all_on:
            names, mode = set(tests), None
        elif primary:
            names, mode = (set(tests), None) if whole else (by_tags, tags_arg)
        elif whole:
            names, mode = smoke, "smoke"
        else:
            continue
        light = False
        if arch != "x86_64" and not groups_everywhere and arch not in all_on:
            light = any(group_of(tests[n]) for n in names)
            names = {n for n in names if group_of(tests[n]) is None}
        selection[mid] = (names, mode, light)
        linux |= any("linux" in tests[n] for n in names)
    return selection, linux, tags_arg


def shards(mid, names, tests, tags_arg):
    """The boots of one machine: the tests outside the long groups in one
    (by tags, or everything but the groups), each group with a selected
    test in its own, whole."""
    machine = next(m for m in MACHINES if m[0] == mid)
    arch_word = machine[1]
    slow = 1.5 if machine[2] == "riscv64" else 1.0
    out = []

    def boot(shard, selection, budget, disk):
        args = (arch_word + " " if arch_word else "") + "test " + selection + f" --budget {int(BUDGETS[budget] * slow)}"
        out.append({
            "id": f"{mid}-{shard}", "machine": mid, "arch": machine[2], "args": args,
            "gic": machine[3] if machine[2] == "aarch64" else "",
            "iommu": machine[3] if machine[2] == "x86_64" else "",
            "intc": machine[3] if machine[2] == "riscv64" else "",
            # The x86 boots run on the runner's CPU through KVM (`/dev/kvm`,
            # which GitHub's Linux runners expose and the boot job's container
            # takes): the light list in 400 s instead of 670 to 950 under TCG.
            "accel": "kvm" if machine[2] == "x86_64" else "",
            "alpine": "true" if disk else "false", "required": True,
        })

    groups = [] if machine[3] in ("amd", "plic") else [g for g in HEAVY_GROUPS if any(group_of(tests[n]) == g for n in names)]
    light = {n for n in names if group_of(tests[n]) is None}
    skip = ",".join(t for gtags in HEAVY_GROUPS.values() for t in gtags)
    if light:
        # Nothing from the network: the quick budget, and the launcher
        # serves no mirror (`net` skipped); nothing from the Alpine disk:
        # it is neither fetched nor attached (`alpine-disk` skipped).
        quick = not any("net" in tests[n] for n in light)
        disk = any("alpine-disk" in tests[n] for n in light)
        if tags_arg is None:
            boot("light", f"--skip {skip}", "light", disk)
        else:
            skips = skip + (",net" if quick else "") + ("" if disk else ",alpine-disk")
            boot("light", f"--tags {tags_arg} --skip {skips}", "smoke" if quick else "light", disk)
    for g in groups:
        disk = any("alpine-disk" in tests[n] and group_of(tests[n]) == g for n in names)
        # The groups before this one keep the tests they share with it.
        before = [t for other, gtags in HEAVY_GROUPS.items() if other != g for t in gtags
                  if list(HEAVY_GROUPS).index(other) < list(HEAVY_GROUPS).index(g)]
        skips = before + ([] if disk else ["alpine-disk"])
        boot(g, f"--tags {','.join(HEAVY_GROUPS[g])}" + (f" --skip {','.join(skips)}" if skips else ""), g, disk)
    return out


def main(argv):
    everything = False
    event = ""
    full_label = False
    base = head = None
    changed = []
    it = iter(argv)
    for arg in it:
        if arg == "--all":
            everything = True
        elif arg == "--event":
            event = next(it)
        elif arg == "--full-label":
            full_label = True
        elif arg == "--base":
            base = next(it)
        elif arg == "--head":
            head = next(it)
        elif arg == "--changed":
            changed = list(it)
        else:
            sys.exit(f"ci-select.py: bad argument {arg}")
    if event in ("push", "schedule") or full_label:
        everything = True
    if not everything and base and not changed:
        try:
            diff = subprocess.run(
                ["git", "diff", "--name-only", "--merge-base", base, head or "HEAD"],
                cwd=ROOT, check=True, capture_output=True, text=True,
            ).stdout
            changed = [p for p in diff.splitlines() if p]
        except subprocess.CalledProcessError as e:
            print(f"ci-select: git diff failed ({e.stderr.strip()}): running everything")
            everything = True

    ports = load_ports()
    tests = load_tests(ports)
    selection, linux, tags_arg = plan(changed, everything, ports, tests)
    boots = []
    for mid, *_ in MACHINES:
        if mid in selection:
            names, mode, _ = selection[mid]
            boots += shards(mid, names, tests, mode)
    if any(b["arch"] == "aarch64" for b in boots):
        boots.append({
            "id": "rpi3-smoke", "machine": "rpi3", "arch": "aarch64", "args": RPI3_ARGS, "gic": "", "iommu": "", "intc": "", "accel": "",
            "alpine": "false", "required": True,
        })

    lines = []
    if everything:
        lines.append("Every test runs (a push to master, the daily run, a dispatch with full_boot or the `ci:full` label).")
    else:
        lines.append(f"{len(changed)} changed path(s): {' '.join(changed[:20])}{' ...' if len(changed) > 20 else ''}")
        if not boots:
            lines.append("No boot test: documentation only.")
        else:
            lines.append(f"Tags: {tags_arg}")
    for mid, *_ in MACHINES:
        if mid in selection:
            names, mode, light = selection[mid]
            what = ("everything" if mode is None else f"`{mode}`") + (", the light boot alone" if light else "")
            lines.append(f"- {mid}: {len(names)} of {len(tests)} tests ({what}) in {sum(1 for b in boots if b['machine'] == mid)} boot(s)")
    for b in boots:
        lines.append(f"  - `{b['id']}`: `myos {b['args']}`")
    summary = "\n".join(lines)
    print(summary)
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as f:
            f.write("## Boot tests\n\n" + summary + "\n")
    out = os.environ.get("GITHUB_OUTPUT")
    output = f"boots={json.dumps(boots, separators=(',', ':'))}\nfeatures={'linux_compat' if linux else ''}\n"
    if out:
        with open(out, "a", encoding="utf-8") as f:
            f.write(output)
    else:
        sys.stdout.write(output)


if __name__ == "__main__":
    main(sys.argv[1:])
