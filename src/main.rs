use std::io;
use termion::event::Key;
use termion::raw::IntoRawMode;

mod input;

fn main() {
    let mut _stdout = io::stdout().into_raw_mode().unwrap();

    for event in input::get_input() {
        println!("Got: {:?}\r", event);

        if let Ok(input::TuiEvent::KeyEvent(Key::Ctrl('c'))) = event {
            println!("Typed C-c, exiting\r");
            break;
        }
    }
}
