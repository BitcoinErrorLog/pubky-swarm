//! Native executable entry point.

// Prevents an additional console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    pubky_swarm_desktop_lib::run();
}
