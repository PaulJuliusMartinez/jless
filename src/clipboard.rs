use std::error::Error;

use clipboard::{self, ClipboardProvider};

pub trait ObjectSafeClipboardProvider {
    /// Method to get the clipboard contents as a String
    fn get_contents(&mut self) -> Result<String, Box<dyn Error>>;
    /// Method to set the clipboard contents as a String
    fn set_contents(&mut self, contents: String) -> Result<(), Box<dyn Error>>;
}

impl<T: ClipboardProvider> ObjectSafeClipboardProvider for T {
    fn get_contents(&mut self) -> Result<String, Box<dyn Error>> {
        self.get_contents()
    }

    fn set_contents(&mut self, contents: String) -> Result<(), Box<dyn Error>> {
        self.set_contents(contents)
    }
}