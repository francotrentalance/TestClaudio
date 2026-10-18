#!/usr/bin/env python3
"""Writes THIRD-PARTY-NOTICES: every crate in the trainer's dependency graph,
its licence, and the licence texts the crates ship.

    python3 packaging/third-party-notices.py [output]

The default output is packaging/THIRD-PARTY-NOTICES; build-deb.sh installs a
fresh copy in /usr/share/doc/holdem-trainer. Package details come from
`cargo metadata`. Which crates the amd64 Linux build uses comes from
`cargo tree`, because the metadata resolve graph ignores weak features and so
also lists crates the glow build never compiles (wgpu, naga, ...). Works
offline once the crates are downloaded (any cargo build does that).
"""

import json
import os
import re
import subprocess
import sys
import textwrap

TARGET = "x86_64-unknown-linux-gnu"
LICENCE_PREFIXES = ("LICENSE", "LICENCE", "COPYING", "COPYRIGHT", "NOTICE", "UNLICENSE")

# Crates that ship no licence file: what to point to instead.
NOTES = {
    "epaint_default_fonts": "Its fonts are listed in /usr/share/doc/holdem-trainer/copyright, with their licences "
    "in UFL.txt, OFL-NotoEmoji.txt, Hack-Regular.txt and emoji-icon-font-mit-license.txt next to it.",
}

MIT = """\
Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE."""


def cargo(manifest, *args):
    return subprocess.run(["cargo", *args, "--manifest-path", manifest, "--locked"], check=True, capture_output=True, text=True).stdout


def normalise(text):
    lines = [line.rstrip() for line in text.replace("\r\n", "\n").replace("\r", "\n").split("\n")]
    return "\n".join(lines).strip("\n")


def licence_files(directory, package):
    names = sorted(f for f in os.listdir(directory) if f.upper().startswith(LICENCE_PREFIXES))
    paths = []
    for name in names:
        path = os.path.join(directory, name)
        if os.path.isdir(path):
            paths += [os.path.join(path, f) for f in sorted(os.listdir(path)) if os.path.isfile(os.path.join(path, f))]
        elif os.path.isfile(path):
            paths.append(path)
    if package.get("license_file"):
        path = os.path.join(directory, package["license_file"])
        if os.path.isfile(path) and path not in paths:
            paths.append(path)
    return paths


def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    manifest = os.path.join(root, "Cargo.toml")
    output = sys.argv[1] if len(sys.argv) > 1 else os.path.join(root, "packaging", "THIRD-PARTY-NOTICES")

    meta = json.loads(cargo(manifest, "metadata", "--format-version", "1", "--filter-platform", TARGET))
    tree = cargo(manifest, "tree", "-e", "normal,build", "--target", TARGET, "--prefix", "none", "--format", "{p}")
    used = {(t[0], t[1].lstrip("v")) for t in (line.split() for line in tree.splitlines()) if len(t) >= 2}
    # Our own path crates (holdem-trainer, holdem-solver) are covered by the copyright file.
    crates = sorted((p for p in meta["packages"] if (p["name"], p["version"]) in used and p.get("source")), key=lambda p: (p["name"], [(0, int(x), "") if x.isdigit() else (1, 0, x) for x in re.split(r"[.+-]", p["version"])]))

    groups = {}  # licence text -> [(crate, file name)]
    bare = []
    for p in crates:
        directory = os.path.dirname(p["manifest_path"])
        files = licence_files(directory, p)
        if not files:
            bare.append(p)
        for path in files:
            with open(path, encoding="utf-8", errors="replace") as f:
                text = normalise(f.read())
            if text:
                groups.setdefault(text, []).append((f"{p['name']} {p['version']}", os.path.relpath(path, directory)))

    out = []
    out.append("Third-party software in Hold'em Trainer")
    out.append("=======================================")
    out.append("")
    out += textwrap.wrap("/usr/bin/holdem-trainer is built from the Rust crates below: they are linked into it, or run "
                         "at build time to generate code for it. Each is used under the licence it is published with; "
                         "where a crate offers a choice, the expression is given as published. The licence files the "
                         "crates ship follow the list.", 79)
    out.append("")
    out.append(f"Generated from Cargo.lock by packaging/third-party-notices.py ({len(crates)} crates).")
    out.append("")
    out.append("")
    out.append("Crates")
    out.append("------")
    out.append("")
    width = max(len(f"{p['name']} {p['version']}") for p in crates) + 2
    for p in crates:
        out.append(f"{p['name']} {p['version']}".ljust(width) + (p.get("license") or "see licence file"))
    out.append("")
    out.append("")
    if bare:
        out.append("Crates published without a licence file")
        out.append("----------------------------------------")
        out.append("")
        apache = any(text.lstrip().startswith("Apache License") for text in groups)
        out += textwrap.wrap("These crates name their licence in Cargo.toml but ship no licence file. Their copyright "
                             "holders are the authors given. The MIT License text is below"
                             + ("; the Apache License 2.0 text is among the licence files further down." if apache else "."), 79)
        out.append("")
        for p in bare:
            out.append(f"{p['name']} {p['version']}: {p.get('license')}")
            authors = ", ".join(a.split(" <")[0] for a in p.get("authors") or []) or "the authors"
            lines = [f"Copyright (c) {authors}"] + ([f"Source: {p['repository']}"] if p.get("repository") else []) + ([NOTES[p["name"]]] if p["name"] in NOTES else [])
            for line in lines:
                out += textwrap.wrap(line, 79, initial_indent="  ", subsequent_indent="  ", break_on_hyphens=False)
        out.append("")
        out.append("MIT License, for the crates above that offer it:")
        out.append("")
        out.append(MIT)
        out.append("")
        out.append("")
    out.append("Licence files")
    out.append("-------------")
    out.append("")
    out += textwrap.wrap("Each file is given once, after the crates that ship it with exactly this text.", 79)
    for text, users in groups.items():
        out.append("")
        out.append("=" * 79)
        out += textwrap.wrap(", ".join(f"{crate} ({name})" for crate, name in users), 79, break_on_hyphens=False)
        out.append("=" * 79)
        out.append("")
        out.append(text)
    out.append("")

    with open(output, "w", encoding="utf-8") as f:
        f.write("\n".join(out))


if __name__ == "__main__":
    main()
