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

The selected tests run on every machine (bios, uefi, aarch64, aarch64 with
GICv3, riscv64; a change under kernel/src/arch/<arch> runs everything on
that arch's machines and the smoke set on the others), in parallel boots:
the long tests' groups (`ostest`, `python`, `x11`, `alpine`: each a boot
of its own, whole, as its tests build on each other) and one boot with
the rest. Each boot gets a budget from what it runs.

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

# The boot machines: id, the launcher's arch word, the arch, the GIC version.
MACHINES = [
    ("bios", "", "x86_64", ""),
    ("uefi", "uefi", "x86_64", ""),
    ("aarch64", "aarch64", "aarch64", ""),
    ("aarch64-gicv3", "aarch64", "aarch64", "3"),
    ("riscv64", "riscv64", "riscv64", ""),
]
# The long tests' groups, each a boot of its own (a `heavy` test outside
# them, the heavy smoke, stays with the rest).
HEAVY_GROUPS = ["ostest", "python", "x11", "alpine"]
# The budget of a boot in minutes, by what it runs; riscv64 (the slowest
# under TCG) gets half as much again.
BUDGETS = {"light": 20, "smoke": 8, "ostest": 15, "python": 20, "x11": 12, "alpine": 12}
# A module's directory selects these tags (its own name otherwise).
MODULE_TAGS = {
    "console": {"console", "tty"},
    "virtio_net": {"virtio_net", "netfs"},
    "virtio": {"blk", "virtio_net"},
    "virtq": {"blk", "virtio_net"},
    "virtio_blk": {"blk"},
    "nvme": {"blk"},
    "virtio_scsi": {"blk"},
    "xhci": {"usb"},
    "usb_hub": {"usb"},
    "usb_storage": {"usb"},
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
    """The selection per machine: a set of test names, and whether the
    Linux layer must be in the image."""
    tags = {"smoke"}
    all_on = set()  # the arches that run everything
    if everything:
        all_on = {m[2] for m in MACHINES}
    else:
        for path in changed:
            what, arch = classify(path, ports)
            if what is None:
                continue
            if what is Everything:
                all_on |= {m[2] for m in MACHINES} if arch is None else {arch}
            else:
                tags |= what
        if not changed:
            return {}, False
        if all(classify(p, ports)[0] is None for p in changed):
            return {}, False
    selection = {}
    linux = False
    for mid, _, arch, _ in MACHINES:
        if arch in all_on:
            names = set(tests)
        else:
            names = {n for n, t in tests.items() if t & tags}
            # The runner installs the packages before a `pkg` test whatever
            # the selection says: that boot needs the mirror and the time.
            if any("pkg" in tests[n] for n in names):
                names.add("packages")
        selection[mid] = names
        linux |= any("linux" in tests[n] for n in names)
    return selection, linux


def shards(mid, names, tests, tags_arg):
    """The boots of one machine: the tests outside the long groups in one
    (by tags, or everything but the groups), each group with a selected
    test in its own, whole."""
    machine = next(m for m in MACHINES if m[0] == mid)
    arch_word = machine[1]
    slow = 1.5 if machine[2] == "riscv64" else 1.0
    out = []

    def boot(shard, selection, budget):
        args = (arch_word + " " if arch_word else "") + "test " + selection + f" --budget {int(BUDGETS[budget] * slow)}"
        out.append({"id": f"{mid}-{shard}", "machine": mid, "args": args, "gic": machine[3], "required": True})

    groups = [g for g in HEAVY_GROUPS if any(g in tests[n] for n in names)]
    light = {n for n in names if not any(g in tests[n] for g in HEAVY_GROUPS)}
    skip = ",".join(HEAVY_GROUPS)
    if light:
        if tags_arg is None:
            boot("light", f"--skip {skip}", "light")
        else:
            # Nothing from the network: the quick budget, and the launcher
            # serves no mirror and prepares no Alpine disk (`net` skipped).
            quick = not any("net" in tests[n] for n in light)
            boot("light", f"--tags {tags_arg} --skip {skip}{',net' if quick else ''}", "smoke" if quick else "light")
    for g in groups:
        boot(g, f"--tags {g}", g)
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
    selection, linux = plan(changed, everything, ports, tests)
    # The tags the light boots name (the planner's union over the change):
    # everything runs by `--skip heavy` alone.
    tags_arg = None
    if not everything:
        tags = {"smoke"}
        for path in changed:
            what, _ = classify(path, ports)
            if what not in (None, Everything):
                tags |= what
        tags_arg = ",".join(sorted(tags))
    boots = []
    for mid, _, arch, _ in MACHINES:
        names = selection.get(mid, set())
        if not names:
            continue
        machine_all = len(names) == len(tests)
        boots += shards(mid, names, tests, None if machine_all else tags_arg)

    lines = []
    if everything:
        lines.append("Every test runs (a push to master, the daily run, a dispatch with full_boot or the `ci:full` label).")
    else:
        lines.append(f"{len(changed)} changed path(s): {' '.join(changed[:20])}{' ...' if len(changed) > 20 else ''}")
        if not boots:
            lines.append("No boot test: documentation only.")
        else:
            lines.append(f"Tags: {tags_arg}")
    for mid, _, _, _ in MACHINES:
        names = selection.get(mid)
        if names:
            lines.append(f"- {mid}: {len(names)} of {len(tests)} tests in {sum(1 for b in boots if b['machine'] == mid)} boot(s)")
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
