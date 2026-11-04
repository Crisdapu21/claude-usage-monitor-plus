#!/usr/bin/env python3
"""Apply this repo's additions on top of an upstream Claude Code Usage Monitor checkout.

usage: apply.py <upstream_dir> <repository_url>
Prints the version it stamped (upstream X.Y.Z -> X.Y.(Z*100 + REVISION)).
The only upstream files touched are src/main.rs (three anchored lines),
Cargo.toml (version, repository) and Cargo.lock (root version).
"""
import pathlib, re, shutil, sys

root = pathlib.Path(__file__).resolve().parent.parent
up = pathlib.Path(sys.argv[1]).resolve()
repo_url = sys.argv[2]

def fail(msg):
    sys.exit(f"apply.py: {msg}. Upstream changed main.rs/Cargo.toml in a way this script does not expect.")

# 1. Module sources.
dst = up / "src" / "activity"
if dst.exists():
    shutil.rmtree(dst)
shutil.copytree(root / "overlay" / "src" / "activity", dst)

# 2. main.rs hooks.
main = up / "src" / "main.rs"
s = main.read_text(encoding="utf-8")
if "mod activity;" not in s:
    s, n = re.subn(r"^(mod \w+;)", r"mod activity;\n\1", s, count=1, flags=re.M)
    if n != 1: fail("no 'mod' line in main.rs")
if "activity::handle_cli_mode" not in s:
    anchor = re.search(r"^([ \t]*)let args: Vec<String> = std::env::args\(\)\.collect\(\);\n", s, flags=re.M)
    if not anchor: fail("args line not found in main.rs")
    ind = anchor.group(1)
    block = (f"{ind}if let Some(exit_code) = activity::handle_cli_mode(&args) {{\n"
             f"{ind}    std::process::exit(exit_code);\n{ind}}}\n")
    s = s[:anchor.end()] + block + s[anchor.end():]
if "activity::start()" not in s:
    idx = s.rfind("window::run();")
    if idx < 0: fail("window::run() not found in main.rs")
    line_start = s.rfind("\n", 0, idx) + 1
    ind = s[line_start:idx]
    s = s[:line_start] + f"{ind}activity::start();\n" + s[line_start:]
main.write_text(s, encoding="utf-8")

# 3. Version and update source.
rev = int((root / "overlay" / "REVISION").read_text().strip())
cargo = up / "Cargo.toml"
c = cargo.read_text(encoding="utf-8")
m = re.search(r'^version = "(\d+)\.(\d+)\.(\d+)"', c, flags=re.M)
if not m: fail("package version not found")
major, minor, patch = map(int, m.groups())
version = f"{major}.{minor}.{patch * 100 + rev}"
c = c[:m.start()] + f'version = "{version}"' + c[m.end():]
c, n = re.subn(r'^repository = ".*"', f'repository = "{repo_url}"', c, count=1, flags=re.M)
if n != 1: fail("repository key not found")
cargo.write_text(c, encoding="utf-8")
lock = up / "Cargo.lock"
l = lock.read_text(encoding="utf-8")
l, n = re.subn(r'(name = "claude-code-usage-monitor"\nversion = )"[^"]+"', rf'\1"{version}"', l, count=1)
if n != 1: fail("root package not found in Cargo.lock")
lock.write_text(l, encoding="utf-8")
print(version)
