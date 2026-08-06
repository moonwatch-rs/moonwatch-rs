#!/usr/bin/env python3
import os.path as op
import os
import shutil
import subprocess
import sys

build_dir = op.abspath(op.dirname(__file__))
install_dir = op.expanduser("~/.moonwatch-rs")

print("Testing availability of dependencies (gnome-screensaver-command, xprintidle, xdotool)")
for cmd in (["gnome-screensaver", "-h"], ["xprintidle", "-v"], ["xdotool", "-v"]):
    try:
        subprocess.check_call(cmd)
    except subprocess.CalledProcessError:
        print("Warning -", cmd, "failed, please install it before using moonwatch-rs")
        sys.exit(1)

print("Testing availability of tray icon libraries (optional)")
try:
    shared_libs = subprocess.check_output(["ldconfig", "-p"]).decode("utf-8", "replace")
except (OSError, subprocess.CalledProcessError):
    shared_libs = None
    print("  could not run ldconfig, skipping check")

if shared_libs is not None:
    if "libgtk-3.so.0" not in shared_libs:
        print("  missing libgtk-3 (Debian/Ubuntu: libgtk-3-0)")
    if not any(lib in shared_libs for lib in ("libayatana-appindicator3.so.1",
                                              "libappindicator3.so.1")):
        print("  missing libappindicator (Debian/Ubuntu: libayatana-appindicator3-1)")

print("  note: moonwatcher runs fine without these, it just has no tray icon;")
print("        on GNOME the icon also needs the 'AppIndicator and KStatusNotifierItem")
print("        Support' shell extension (shipped by default on Ubuntu)")

print("Stopping moonwatch-rs service")
rv = subprocess.call(["systemctl", "--user", "stop", "moonwatch-rs"])
print("systemctl returned code", rv)

print("Installing into", install_dir)
if not os.path.exists(install_dir):
    print("Creating directory", install_dir)
    os.makedirs(install_dir)

shutil.copy(op.join(build_dir, "moonwatcher"), install_dir)
if op.exists(op.join(install_dir, "config.json")):
    print("config.json already exists, not copying default")
else:
    print("copying default config.json")
    shutil.copy(op.join(build_dir, "config.json"), install_dir)

print("Setting up Systemd user service")
systemd_user_dir = op.expanduser("~/.config/systemd/user")
os.makedirs(systemd_user_dir, exist_ok=True)
shutil.copy(op.join(build_dir, "moonwatch-rs.service"), systemd_user_dir)

cmd = ["systemctl", "--user", "enable", "moonwatch-rs"]
print("Enabling moonwatch-rs service:", cmd)
subprocess.check_call(cmd)

print("Starting moonwatch-rs service")
rv = subprocess.call(["systemctl", "--user", "start", "moonwatch-rs"])
print("systemctl returned code", rv)
