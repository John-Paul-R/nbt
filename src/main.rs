use std::io::{self, Cursor, IsTerminal, Read, Write};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use quartz_nbt::io::Flavor;
use quartz_nbt::{NbtCompound, NbtTag};

fn main() -> Result<()> {
    let args = parse_args()?;
    let palette = Palette::resolve(args.color);

    let mut raw = Vec::new();
    match args.file {
        Some(ref path) => std::fs::File::open(path)
            .with_context(|| format!("failed to open {path:?}"))?
            .read_to_end(&mut raw)
            .with_context(|| format!("failed to read {path:?}"))?,
        None => io::stdin()
            .read_to_end(&mut raw)
            .context("failed to read stdin")?,
    };

    // If the input looks like SNBT text, parse it as such.
    // Otherwise try binary NBT, auto-detecting gzip compression.
    let compound: NbtCompound = if looks_like_snbt(&raw) {
        let text = std::str::from_utf8(&raw).context("SNBT input is not valid UTF-8")?;
        quartz_nbt::snbt::parse(text).context("failed to parse SNBT")?
    } else if raw.starts_with(&[0x1f, 0x8b]) {
        quartz_nbt::io::read_nbt(&mut Cursor::new(&raw), Flavor::GzCompressed)
            .context("failed to parse gzip-compressed NBT")?
            .0
    } else {
        quartz_nbt::io::read_nbt(&mut Cursor::new(&raw), Flavor::Uncompressed)
            .context("failed to parse binary NBT")?
            .0
    };

    let value = NbtTag::Compound(compound);
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    print_tag(&mut out, &value, 0, &palette)?;
    writeln!(out)?;

    Ok(())
}

// -- SNBT detection --

// Returns true when the bytes look like SNBT text (UTF-8 starting with '{').
fn looks_like_snbt(data: &[u8]) -> bool {
    let first_non_ws = data.iter().position(|&b| !b.is_ascii_whitespace());
    match first_non_ws.map(|i| data[i]) {
        Some(b'{') => std::str::from_utf8(data).is_ok(),
        _ => false,
    }
}

// -- Arg parsing --

struct Args {
    color: ColorMode,
    file:  Option<PathBuf>,
}

// Accepts: nbt-printer [--color=auto|always|never] [file]
// The positional arg and the flag can appear in any order.
fn parse_args() -> Result<Args> {
    let mut color = ColorMode::Auto;
    let mut file: Option<PathBuf> = None;

    for arg in std::env::args().skip(1) {
        if let Some(v) = arg.strip_prefix("--color=") {
            color = parse_color_value(v)?;
        } else if arg == "--color" {
            color = ColorMode::Always;
        } else if arg.starts_with('-') {
            bail!("unknown flag: {arg}\nusage: nbt-printer [--color=auto|always|never] [file]");
        } else if file.is_none() {
            file = Some(PathBuf::from(&arg));
        } else {
            bail!("unexpected argument: {arg}\nusage: nbt-printer [--color=auto|always|never] [file]");
        }
    }

    Ok(Args { color, file })
}

fn parse_color_value(v: &str) -> Result<ColorMode> {
    match v {
        "auto"   => Ok(ColorMode::Auto),
        "always" => Ok(ColorMode::Always),
        "never"  => Ok(ColorMode::Never),
        other    => bail!("invalid --color value: {other} (expected auto, always, or never)"),
    }
}

// -- Color mode / palette --

enum ColorMode { Auto, Always, Never }

// ANSI escape codes for each semantic role. All fields are empty when color is off.
struct Palette {
    key:    &'static str, // compound keys
    string: &'static str, // string values
    number: &'static str, // numeric digits
    suffix: &'static str, // type suffix letters (b, s, L, f, d)
    punct:  &'static str, // brackets, braces, commas, colons
    array:  &'static str, // [B; / [I; / [L; prefix letter
    reset:  &'static str,
}

impl Palette {
    fn resolve(mode: ColorMode) -> Self {
        let use_color = match mode {
            ColorMode::Always => true,
            ColorMode::Never  => false,
            ColorMode::Auto   => {
                // Respect the NO_COLOR convention (https://no-color.org).
                std::env::var_os("NO_COLOR").is_none() && io::stdout().is_terminal()
            }
        };

        if use_color {
            Self {
                key:    "\x1b[1;34m",  // bold blue  - same as jq object keys
                string: "\x1b[0;32m",  // green      - same as jq strings
                number: "\x1b[0;36m",  // cyan
                suffix: "\x1b[2;36m",  // dim cyan   - de-emphasised type tags
                punct:  "\x1b[1m",     // bold       - same as jq structural chars
                array:  "\x1b[0;35m",  // magenta    - array-type prefix letter
                reset:  "\x1b[0m",
            }
        } else {
            Self { key: "", string: "", number: "", suffix: "", punct: "", array: "", reset: "" }
        }
    }
}

// -- SNBT formatting --

const INDENT: &str = "  ";

fn print_tag(w: &mut impl Write, tag: &NbtTag, depth: usize, p: &Palette) -> Result<()> {
    let r = p.reset;
    match tag {
        NbtTag::Byte(b)   => write!(w, "{}{b}{r}{}b{r}", p.number, p.suffix)?,
        NbtTag::Short(s)  => write!(w, "{}{s}{r}{}s{r}", p.number, p.suffix)?,
        NbtTag::Int(i)    => write!(w, "{}{i}{r}", p.number)?,
        NbtTag::Long(l)   => write!(w, "{}{l}{r}{}L{r}", p.number, p.suffix)?,
        NbtTag::Float(f)  => write!(w, "{}{}{r}{}f{r}", p.number, format_float(*f as f64), p.suffix)?,
        NbtTag::Double(d) => write!(w, "{}{}{r}{}d{r}", p.number, format_float(*d), p.suffix)?,
        NbtTag::String(s) => write!(w, "{}{}{r}", p.string, quote_string(s))?,

        // Arrays stay on one line - they tend to be large and uniform.
        NbtTag::ByteArray(arr) => {
            write!(w, "{}[{r}{}B{r}{};{r}", p.punct, p.array, p.punct)?;
            for (i, b) in arr.iter().enumerate() {
                if i > 0 { write!(w, "{},{r}", p.punct)?; }
                write!(w, " {}{b}{r}{}b{r}", p.number, p.suffix)?;
            }
            write!(w, " {}]{r}", p.punct)?;
        }
        NbtTag::IntArray(arr) => {
            write!(w, "{}[{r}{}I{r}{};{r}", p.punct, p.array, p.punct)?;
            for (i, v) in arr.iter().enumerate() {
                if i > 0 { write!(w, "{},{r}", p.punct)?; }
                write!(w, " {}{v}{r}", p.number)?;
            }
            write!(w, " {}]{r}", p.punct)?;
        }
        NbtTag::LongArray(arr) => {
            write!(w, "{}[{r}{}L{r}{};{r}", p.punct, p.array, p.punct)?;
            for (i, v) in arr.iter().enumerate() {
                if i > 0 { write!(w, "{},{r}", p.punct)?; }
                write!(w, " {}{v}{r}{}L{r}", p.number, p.suffix)?;
            }
            write!(w, " {}]{r}", p.punct)?;
        }

        NbtTag::List(list) => {
            if list.is_empty() {
                write!(w, "{}[]{r}", p.punct)?;
                return Ok(());
            }
            writeln!(w, "{}[{r}", p.punct)?;
            let child_depth = depth + 1;
            let prefix = INDENT.repeat(child_depth);
            let close  = INDENT.repeat(depth);
            for (i, item) in list.iter().enumerate() {
                write!(w, "{prefix}")?;
                print_tag(w, item, child_depth, p)?;
                if i + 1 < list.len() { write!(w, "{},{r}", p.punct)?; }
                writeln!(w)?;
            }
            write!(w, "{close}{}]{r}", p.punct)?;
        }

        NbtTag::Compound(map) => {
            print_compound(w, map, depth, p)?;
        }
    }
    Ok(())
}

fn print_compound(w: &mut impl Write, map: &NbtCompound, depth: usize, p: &Palette) -> Result<()> {
    let r = p.reset;
    let inner = map.inner();
    if inner.is_empty() {
        write!(w, "{}{{}}{r}", p.punct)?;
        return Ok(());
    }
    writeln!(w, "{}{{{r}", p.punct)?;
    let child_depth = depth + 1;
    let prefix = INDENT.repeat(child_depth);
    let close  = INDENT.repeat(depth);
    // Sort keys for deterministic output.
    let mut keys: Vec<&String> = inner.keys().collect();
    keys.sort();
    for (i, key) in keys.iter().enumerate() {
        let val = &inner[*key];
        write!(w, "{prefix}{}{}{r}{}: ", p.key, quote_key(key), p.punct)?;
        print_tag(w, val, child_depth, p)?;
        if i + 1 < inner.len() { write!(w, "{},{r}", p.punct)?; }
        writeln!(w)?;
    }
    write!(w, "{close}{}}}{r}", p.punct)?;
    Ok(())
}

// -- String/key helpers --

// Quotes a compound key. Simple identifiers don't need quotes.
fn quote_key(key: &str) -> String {
    if !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.') {
        key.to_owned()
    } else {
        quote_string(key)
    }
}

// Wraps a string in the appropriate quotes, escaping as needed.
// Mirrors vanilla: prefer double quotes; switch to single quotes if the string
// contains a double quote but no single quote.
fn quote_string(s: &str) -> String {
    let has_double = s.contains('"');
    let has_single = s.contains('\'');
    if has_double && !has_single {
        format!("'{s}'")
    } else {
        let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    }
}

// Formats a float/double without redundant trailing zeros, but always with a
// decimal point so the type is visually unambiguous from integers.
fn format_float(v: f64) -> String {
    if v.is_nan()      { return "NaN".to_owned(); }
    if v.is_infinite() { return if v > 0.0 { "Infinity".to_owned() } else { "-Infinity".to_owned() }; }
    let s = format!("{v}");
    if s.contains('.') || s.contains('e') || s.contains('E') { s } else { format!("{s}.0") }
}
