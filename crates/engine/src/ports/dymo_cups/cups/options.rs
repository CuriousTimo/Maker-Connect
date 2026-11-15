// Ported from CUPS 2.4.20 (reference/cups-2.4.20/cups/options.c). Copyright © 2020-2024 by OpenPrinting,
// 2007-2017 by Apple Inc. Apache-2.0 with the GPL/LGPL exception (reference/cups-2.4.20/LICENSE).

//! `cupsParseOptions` (`options.c:260-465`): the filter's `argv[5]`.

/// `cups_option_t`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CupsOption {
    pub name: String,
    pub value: String,
}

/// `_cups_isspace`: `\f \n \r \t \v` and space.
fn is_space(c: u8) -> bool {
    matches!(c, b' ' | 0x0c | b'\n' | b'\r' | b'\t' | 0x0b)
}

/// `cupsAddOption`: "adds or replaces", the name compared without case.
fn add_option(name: &str, value: &str, options: &mut Vec<CupsOption>) {
    for o in options.iter_mut() {
        if o.name.eq_ignore_ascii_case(name) {
            o.value = value.to_owned();
            return;
        }
    }
    options.push(CupsOption { name: name.to_owned(), value: value.to_owned() });
}

/// `cupsParseOptions(arg, num_options, &options)` (:260-465), `options.c`: the options found added to `options`.
pub fn cups_parse_options(arg: &str, options: &mut Vec<CupsOption>) {
    let mut copyarg: Vec<u8> = arg.as_bytes().to_vec();
    copyarg.push(0);

    // :312-326 `{name=value ... name=value}`
    let mut p: usize;
    if copyarg[0] == b'{' {
        let len = copyarg.len() - 1;
        if len > 0 && len - 1 > 0 && copyarg[len - 1] == b'}' {
            copyarg[len - 1] = 0;
            p = 1;
        } else {
            p = 0;
        }
    } else {
        p = 0;
    }

    // skip leading spaces
    while is_space(copyarg[p]) {
        p += 1;
    }

    // :330 loop through the string
    while copyarg[p] != 0 {
        // get the name up to a SPACE, =, or end-of-string
        let name_start = p;
        while !b"\x0c\n\r\t\x0b =".contains(&copyarg[p]) && copyarg[p] != 0 {
            p += 1;
        }

        // avoid an empty name
        if p == name_start {
            break;
        }
        let name_end = p;

        // skip trailing spaces
        while is_space(copyarg[p]) {
            copyarg[p] = 0;
            p += 1;
        }

        let sep = copyarg[p];
        if sep == b'=' {
            copyarg[p] = 0;
            p += 1;
        }
        let name = String::from_utf8_lossy(&copyarg[name_start..name_end]).into_owned();

        if sep != b'=' {
            // boolean option
            if name.len() >= 2 && name[..2].eq_ignore_ascii_case("no") {
                add_option(&name[2..], "false", options);
            } else {
                add_option(&name, "true", options);
            }
            continue;
        }

        // remove = and parse the value
        let value_start = p;

        while copyarg[p] != 0 && !is_space(copyarg[p]) {
            if copyarg[p] == b',' {
                p += 1;
            } else if copyarg[p] == b'\'' || copyarg[p] == b'"' {
                // quoted string constant
                let quote = copyarg[p];
                copyarg.remove(p); // _cups_strcpy(ptr, ptr + 1)

                while copyarg[p] != quote && copyarg[p] != 0 {
                    if copyarg[p] == b'\\' && copyarg[p + 1] != 0 {
                        copyarg.remove(p);
                    }
                    p += 1;
                }

                if copyarg[p] != 0 {
                    copyarg.remove(p);
                }
            } else if copyarg[p] == b'{' {
                // collection value
                let mut depth = 0;
                while copyarg[p] != 0 {
                    if copyarg[p] == b'{' {
                        depth += 1;
                    } else if copyarg[p] == b'}' {
                        depth -= 1;
                        if depth == 0 {
                            p += 1;
                            break;
                        }
                    } else if copyarg[p] == b'\\' && copyarg[p + 1] != 0 {
                        copyarg.remove(p);
                    }
                    p += 1;
                }
            } else {
                // normal space-delimited string
                while copyarg[p] != 0 && !is_space(copyarg[p]) {
                    if copyarg[p] == b'\\' && copyarg[p + 1] != 0 {
                        copyarg.remove(p);
                    }
                    p += 1;
                }
            }
        }

        let value_end = p;
        if copyarg[p] != 0 {
            copyarg[p] = 0;
            p += 1;
        }

        // skip trailing whitespace
        while is_space(copyarg[p]) {
            p += 1;
        }

        // add the string value
        let value = String::from_utf8_lossy(&copyarg[value_start..value_end]).into_owned();
        add_option(&name, &value, options);
    }
}

/// `cupsGetOption` (`options.c`): the value of the named option, the name compared without case.
pub fn cups_get_option<'a>(name: &str, options: &'a [CupsOption]) -> Option<&'a str> {
    options.iter().find(|o| o.name.eq_ignore_ascii_case(name)).map(|o| o.value.as_str())
}
