#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

pub mod error;
mod nuitka;
mod ui;

fn main() {
    ui::run();
}
