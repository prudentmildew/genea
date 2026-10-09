/// The system clipboard (the macOS general pasteboard), plain text only.
///
/// Unlike the other host effects, the core uses the clipboard on the main
/// thread, because Cut, Copy and Paste apply synchronously like every other
/// edit. Implementations must answer quickly and never block on another
/// process for long.
pub trait Clipboard: Send + Sync {
    /// The clipboard's text, or `None` if it holds no text.
    fn read_text(&self) -> Option<String>;

    /// Replaces the clipboard's contents with `text`.
    fn write_text(&self, text: &str);
}
