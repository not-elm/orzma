//! The dynamic foreground and background an `OSC 10` or `OSC 11` sets
//! and queries, and an `OSC 110` or `OSC 111` resets.

use crate::device::color::Rgb;
use crate::interpreter::osc::{OscTerminator, rgb_spec};

/// One of the dynamic colors this terminal carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DynamicColor {
    /// The default foreground `SGR 39` resolves to (`OSC 10`).
    Foreground,
    /// The default background `SGR 49` resolves to (`OSC 11`).
    Background,
}

/// One request an `OSC 10`, `OSC 11`, `OSC 110`, or `OSC 111` makes of
/// a dynamic color, decoded before the device is touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DynamicColorRequest {
    /// Sets `target` to `color`.
    Set { target: DynamicColor, color: Rgb },
    /// Reports the colour `target` holds.
    Query { target: DynamicColor },
    /// Returns `target` to its default.
    Reset { target: DynamicColor },
}

impl DynamicColor {
    /// The dynamic color a `Ps` of an `OSC 10` through `OSC 19` names;
    /// `None` for one this terminal does not carry.
    fn from_code(code: usize) -> Option<Self> {
        match code {
            10 => Some(Self::Foreground),
            11 => Some(Self::Background),
            _ => None,
        }
    }

    /// The `Ps` that names this color when set or queried.
    const fn code(self) -> u8 {
        match self {
            Self::Foreground => 10,
            Self::Background => 11,
        }
    }
}

impl DynamicColorRequest {
    /// The dynamic-color requests one operating system command carries,
    /// in the order they appear; empty for every other command.
    ///
    /// The value after the number sets or queries the color that number
    /// names, and "Each successive parameter changes the next color in
    /// the list" (xterm-ctlseqs.pdf p.39), so a command starting at
    /// `OSC 10` reaches the background with its second value. A `?` in
    /// place of the value asks for that color instead of setting it.
    /// `OSC 110` and `OSC 111` take no value.
    ///
    /// # Invariants
    ///
    /// A value that cannot be read is dropped on its own and the values
    /// after it still decode. A chain ends at the first color this
    /// terminal does not carry, and a command whose number names one
    /// decodes to nothing. A reset ignores whatever follows its number.
    pub fn parse(params: &[&[u8]]) -> Vec<Self> {
        match params {
            [b"10", specs @ ..] => Self::chain(DynamicColor::Foreground, specs),
            [b"11", specs @ ..] => Self::chain(DynamicColor::Background, specs),
            [b"110", ..] => vec![Self::Reset {
                target: DynamicColor::Foreground,
            }],
            [b"111", ..] => vec![Self::Reset {
                target: DynamicColor::Background,
            }],
            _ => Vec::new(),
        }
    }

    /// The requests the values of one command make, `first` naming the
    /// color the leading value addresses and each later value
    /// addressing the next color.
    fn chain(first: DynamicColor, specs: &[&[u8]]) -> Vec<Self> {
        specs
            .iter()
            .enumerate()
            .map_while(|(offset, spec)| {
                let target = DynamicColor::from_code(usize::from(first.code()) + offset)?;
                Some((target, *spec))
            })
            .filter_map(|(target, spec)| Self::from_spec(target, spec))
            .collect()
    }

    /// The request one dynamic colour's value makes; `None` when the
    /// value is neither `?` nor a colour spec this terminal reads.
    fn from_spec(target: DynamicColor, spec: &[u8]) -> Option<Self> {
        if spec == b"?" {
            return Some(Self::Query { target });
        }
        Rgb::from_color_spec(spec).map(|color| Self::Set { target, color })
    }
}

/// The reply an `OSC 10 ; ?` or `OSC 11 ; ?` owes: the same command
/// with `target`'s colour spelled as `rgb:`, closed the way the query
/// was.
///
/// The reply names `target`'s own colour number, so each reply of a
/// chained query sets back the colour it reports rather than the one
/// the query started at.
pub(crate) fn dynamic_color_reply(
    target: DynamicColor,
    color: Rgb,
    terminator: OscTerminator,
) -> Vec<u8> {
    let spec = rgb_spec(color);
    let code = target.code();
    let end = terminator.as_str();
    format!("\x1b]{code};{spec}{end}").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }

    /// Asserts that an `OSC 10` carrying a colour spec decodes to a set
    /// of the foreground.
    ///
    /// Case: a colour-scheme script recolors the terminal's text with
    /// `OSC 10 ; rgb:12/34/56`.
    #[test]
    fn an_osc_10_spec_decodes_to_a_foreground_set() {
        assert_eq!(
            DynamicColorRequest::parse(&[b"10", b"rgb:12/34/56"]),
            vec![DynamicColorRequest::Set {
                target: DynamicColor::Foreground,
                color: rgb(0x12, 0x34, 0x56)
            }]
        );
    }

    /// Asserts that an `OSC 11` carrying a colour spec decodes to a set
    /// of the background.
    ///
    /// Case: a colour-scheme script gives the terminal a dark grey
    /// ground with `OSC 11 ; #202020`.
    #[test]
    fn an_osc_11_spec_decodes_to_a_background_set() {
        assert_eq!(
            DynamicColorRequest::parse(&[b"11", b"#202020"]),
            vec![DynamicColorRequest::Set {
                target: DynamicColor::Background,
                color: rgb(0x20, 0x20, 0x20)
            }]
        );
    }

    /// Asserts that a `?` in place of the spec decodes to a query of
    /// that color.
    ///
    /// Case: nvim asks for the background at startup so that it can
    /// decide whether to set its `background` option to dark or light.
    #[test]
    fn a_question_mark_decodes_to_a_query() {
        assert_eq!(
            DynamicColorRequest::parse(&[b"11", b"?"]),
            vec![DynamicColorRequest::Query {
                target: DynamicColor::Background
            }]
        );
    }

    /// Asserts that the values after the first change each successive
    /// color, so a command starting at `OSC 10` sets the foreground and
    /// then the background.
    ///
    /// Case: a theme script recolors both the text and the ground of
    /// the terminal in a single command.
    #[test]
    fn successive_values_change_successive_colors() {
        assert_eq!(
            DynamicColorRequest::parse(&[b"10", b"rgb:ff/ff/ff", b"rgb:00/00/00"]),
            vec![
                DynamicColorRequest::Set {
                    target: DynamicColor::Foreground,
                    color: rgb(0xff, 0xff, 0xff)
                },
                DynamicColorRequest::Set {
                    target: DynamicColor::Background,
                    color: rgb(0x00, 0x00, 0x00)
                },
            ]
        );
    }

    /// Asserts that a chained query decodes to one query per color.
    ///
    /// Case: a program saves both the text and the ground colour before
    /// it recolors them, so that it can restore each on exit.
    #[test]
    fn a_chained_query_decodes_to_one_query_per_color() {
        assert_eq!(
            DynamicColorRequest::parse(&[b"10", b"?", b"?"]),
            vec![
                DynamicColorRequest::Query {
                    target: DynamicColor::Foreground
                },
                DynamicColorRequest::Query {
                    target: DynamicColor::Background
                },
            ]
        );
    }

    /// Asserts that a chain stops at the first color this terminal does
    /// not carry, so a third value reaches no cursor color.
    ///
    /// Case: a script written for xterm recolors the text, the ground,
    /// and the cursor in one command.
    #[test]
    fn a_chain_stops_at_the_first_color_this_terminal_does_not_carry() {
        assert_eq!(
            DynamicColorRequest::parse(&[b"10", b"rgb:ff/00/00", b"rgb:00/ff/00", b"rgb:00/00/ff"]),
            vec![
                DynamicColorRequest::Set {
                    target: DynamicColor::Foreground,
                    color: rgb(0xff, 0x00, 0x00)
                },
                DynamicColorRequest::Set {
                    target: DynamicColor::Background,
                    color: rgb(0x00, 0xff, 0x00)
                },
            ]
        );
    }

    /// Asserts that an `OSC 12` decodes to nothing, its starting point
    /// being a color this terminal does not carry.
    ///
    /// Case: nvim recolors the cursor for insert mode through its
    /// `guicursor` option.
    #[test]
    fn an_osc_12_decodes_to_nothing() {
        assert!(DynamicColorRequest::parse(&[b"12", b"rgb:ff/00/00"]).is_empty());
    }

    /// Asserts that a command carrying no value at all decodes to
    /// nothing.
    ///
    /// Case: a program emits a bare `OSC 10`, omitting the separator
    /// and the spec after it.
    #[test]
    fn a_command_without_a_value_decodes_to_nothing() {
        assert!(DynamicColorRequest::parse(&[b"10"]).is_empty());
    }

    /// Asserts that an `OSC 110` decodes to a reset of the foreground.
    ///
    /// Case: a program restores the text colour it changed before it
    /// exits.
    #[test]
    fn an_osc_110_decodes_to_a_foreground_reset() {
        assert_eq!(
            DynamicColorRequest::parse(&[b"110"]),
            vec![DynamicColorRequest::Reset {
                target: DynamicColor::Foreground
            }]
        );
    }

    /// Asserts that an `OSC 111` decodes to a reset of the background.
    ///
    /// Case: a program restores the ground colour it changed before it
    /// exits.
    #[test]
    fn an_osc_111_decodes_to_a_background_reset() {
        assert_eq!(
            DynamicColorRequest::parse(&[b"111"]),
            vec![DynamicColorRequest::Reset {
                target: DynamicColor::Background
            }]
        );
    }

    /// Asserts that a reset reads and discards the parameters after its
    /// number, resetting only the color that number names rather than
    /// chaining to the next one.
    ///
    /// Case: a script restores both colours with `OSC 110 ; 11`,
    /// borrowing the `OSC 104` spelling that does take colour numbers.
    #[test]
    fn a_reset_ignores_the_parameters_after_its_number() {
        assert_eq!(
            DynamicColorRequest::parse(&[b"110", b"11"]),
            vec![DynamicColorRequest::Reset {
                target: DynamicColor::Foreground
            }]
        );
    }

    /// Asserts that the indexed-palette commands and the window title
    /// decode to no dynamic-color request.
    ///
    /// Case: a theme script recolors an indexed slot and a shell sets
    /// its title, and both reach this decoder on their way through the
    /// operating system command dispatch.
    #[test]
    fn the_indexed_palette_commands_decode_to_no_dynamic_color_request() {
        assert!(DynamicColorRequest::parse(&[b"4", b"1", b"rgb:ff/00/00"]).is_empty());
        assert!(DynamicColorRequest::parse(&[b"104"]).is_empty());
        assert!(DynamicColorRequest::parse(&[b"0", b"title"]).is_empty());
        assert!(DynamicColorRequest::parse(&[]).is_empty());
    }

    /// Asserts that a spec that cannot be read drops only its own
    /// position rather than ending the command, so the positions after
    /// it still decode.
    ///
    /// Case: a script written for xterm names the foreground `red`, a
    /// colour name this terminal does not know, before giving the
    /// background an `rgb:` spec.
    #[test]
    fn an_unreadable_spec_drops_only_its_own_position() {
        assert_eq!(
            DynamicColorRequest::parse(&[b"10", b"red", b"rgb:00/00/ff"]),
            vec![DynamicColorRequest::Set {
                target: DynamicColor::Background,
                color: rgb(0x00, 0x00, 0xff)
            }]
        );
    }

    /// Asserts that a foreground reply names its own colour number and
    /// writes each channel as the four hex digits an eight-bit value
    /// scales to, closing with the terminator the query used.
    ///
    /// Case: a program asks for the terminal's text colour with a
    /// BEL-closed `OSC 10 ; ?`.
    #[test]
    fn a_foreground_reply_names_its_own_number() {
        assert_eq!(
            dynamic_color_reply(
                DynamicColor::Foreground,
                rgb(0xcd, 0x00, 0x00),
                OscTerminator::Bel
            ),
            b"\x1b]10;rgb:cdcd/0000/0000\x07"
        );
    }

    /// Asserts that a background reply names its own colour number
    /// rather than the number of the color the query started at.
    ///
    /// Case: nvim asks for the background at startup, and a theme
    /// script asks for both colours in one chained query.
    #[test]
    fn a_background_reply_names_its_own_number() {
        assert_eq!(
            dynamic_color_reply(
                DynamicColor::Background,
                rgb(0x20, 0x20, 0x20),
                OscTerminator::Bel
            ),
            b"\x1b]11;rgb:2020/2020/2020\x07"
        );
    }

    /// Asserts that a query closed with the string terminator is
    /// answered with its seven-bit form.
    ///
    /// Case: a terminfo-driven program closes its query with `ESC \`
    /// instead of BEL.
    #[test]
    fn an_st_closed_query_is_answered_with_the_seven_bit_string_terminator() {
        assert_eq!(
            dynamic_color_reply(
                DynamicColor::Foreground,
                rgb(0xab, 0x12, 0xef),
                OscTerminator::St
            ),
            b"\x1b]10;rgb:abab/1212/efef\x1b\\"
        );
    }

    /// Asserts that a reply decodes back to the color and the target it
    /// reports, so replaying it sets the same color again.
    ///
    /// Case: a program saves a dynamic colour with a query and later
    /// restores it by sending the reply back to the terminal.
    #[test]
    fn a_reply_reads_back_to_the_color_it_reports() {
        for target in [DynamicColor::Foreground, DynamicColor::Background] {
            for channel in [0x00, 0x01, 0x7f, 0x80, 0xcd, 0xfe, 0xff] {
                let color = rgb(channel, channel, channel);
                let reply = dynamic_color_reply(target, color, OscTerminator::Bel);
                let body = reply
                    .strip_prefix(b"\x1b]".as_slice())
                    .and_then(|rest| rest.strip_suffix(b"\x07".as_slice()))
                    .expect("the reply is framed as OSC Ps ; spec BEL");
                let params: Vec<&[u8]> = body.split(|byte| *byte == b';').collect();
                assert_eq!(
                    DynamicColorRequest::parse(&params),
                    vec![DynamicColorRequest::Set { target, color }]
                );
            }
        }
    }
}
