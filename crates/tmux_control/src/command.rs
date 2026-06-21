//! The `TmuxCommand` trait: renders a typed tmux command to its raw wire string.

/// A typed tmux control-mode command that renders to its raw wire string.
pub trait TmuxCommand {
    /// Consumes the command and returns the raw control-mode command line.
    fn into_raw_command(self) -> String;
}

/// Escape hatch: a reference to anything string-like is an already-rendered command.
impl<T: AsRef<str> + ?Sized> TmuxCommand for &T {
    fn into_raw_command(self) -> String {
        self.as_ref().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn str_slice_renders_itself() {
        assert_eq!("detach-client".into_raw_command(), "detach-client");
    }

    #[test]
    fn string_ref_renders_its_contents() {
        let owned = String::from("kill-server");
        assert_eq!((&owned).into_raw_command(), "kill-server");
    }

    #[test]
    fn format_temporary_renders_its_contents() {
        let target = "%3";
        assert_eq!(
            (&format!("select-pane -t {target}")).into_raw_command(),
            "select-pane -t %3"
        );
    }
}
