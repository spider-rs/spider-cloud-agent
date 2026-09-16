//! Where results go, and in what shape.
//!
//! Three shapes and three destinations, and they combine. Text is the content
//! with no wrapper. JSON buffers the run into one document. NDJSON writes a
//! line per item and flushes it, so a caller reading the pipe has the first
//! page before the last one is fetched.
//!
//! A directory destination writes one file per page, named from the address,
//! so a second run over the same addresses produces the same names and the two
//! runs diff. The name is built from the host and path with everything outside
//! `a-z0-9-` replaced, so it carries no separator and cannot climb out of the
//! directory it was given.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use serde_json::Value;
use url::Url;

use crate::cli::Format;
use crate::exit::{Code, Failure, Run};

/// One thing to write.
pub struct Item {
    /// The structured form, used by JSON and NDJSON.
    pub value: Value,
    /// The plain form, used by text output.
    pub text: Option<String>,
    /// The binary form, which wins over the text when it is there.
    pub bytes: Option<Vec<u8>>,
    /// The address this came from, which names the file in a directory.
    pub url: Option<Url>,
    /// The extension a directory destination gives this item under text
    /// output.
    pub extension: &'static str,
}

impl Item {
    /// An item that only has a structured form.
    pub fn structured(value: Value) -> Item {
        Item {
            value,
            text: None,
            bytes: None,
            url: None,
            extension: "txt",
        }
    }

    /// Say where it came from.
    pub fn with_url(mut self, url: Url) -> Item {
        self.url = Some(url);
        self
    }

    /// Carry a plain form as well.
    pub fn with_text(mut self, text: String, extension: &'static str) -> Item {
        self.text = Some(text);
        self.extension = extension;
        self
    }
}

/// Where the results are written.
enum Dest {
    /// One stream, stdout or a file.
    Stream(Box<dyn Write>),
    /// One file per item, under this directory.
    Dir(PathBuf),
}

/// How the caller wants files created.
#[derive(Debug, Clone, Copy)]
pub struct FileRules {
    /// Overwrite a file that is already there.
    pub force: bool,
    /// Create the parent directories.
    pub mkdir: bool,
    /// Append rather than replace.
    pub append: bool,
}

/// The writer every command hands its results to.
pub struct Emitter {
    format: Format,
    dest: Dest,
    rules: FileRules,
    buffered: Vec<Value>,
    report: Option<Value>,
    sole: bool,
    count: usize,
}

impl Emitter {
    /// Open the destination the caller named.
    ///
    /// A file that is already there stops the run before anything is fetched,
    /// which is the point: finding out after a crawl that the output was
    /// refused has already spent the credits.
    pub fn open(
        format: Format,
        output: Option<&str>,
        output_dir: Option<&str>,
        rules: FileRules,
    ) -> Run<Emitter> {
        let dest = match (output, output_dir) {
            // A dash names stdin on every flag that reads, so on the flag
            // that writes it names stdout. Read as a file name it would
            // create a file called `-` in the working directory.
            (Some("-"), _) => Dest::Stream(Box::new(std::io::stdout())),
            (Some(path), _) => {
                Dest::Stream(Box::new(BufWriter::new(open_file(Path::new(path), rules)?)))
            }
            (None, Some(dir)) => {
                let dir = PathBuf::from(dir);
                if rules.mkdir {
                    std::fs::create_dir_all(&dir).map_err(|e| {
                        Failure::output(format!("could not create {}: {e}", dir.display()))
                    })?;
                }
                if !dir.is_dir() {
                    return Err(Failure::output(format!(
                        "{} is not a directory. Create it, or pass --mkdir.",
                        dir.display()
                    )));
                }
                Dest::Dir(dir)
            }
            (None, None) => Dest::Stream(Box::new(std::io::stdout())),
        };
        Ok(Emitter {
            format,
            dest,
            rules,
            buffered: Vec::new(),
            report: None,
            sole: false,
            count: 0,
        })
    }

    /// The shape this emitter writes.
    pub fn format(&self) -> Format {
        self.format
    }

    /// Write one item.
    pub fn write(&mut self, item: Item) -> Run<()> {
        self.count += 1;
        match (&mut self.dest, self.format) {
            (Dest::Dir(dir), format) => {
                let name = file_name(item.url.as_ref(), self.count, format, item.extension);
                let path = dir.join(name);
                let mut file = open_file(
                    &path,
                    FileRules {
                        append: false,
                        ..self.rules
                    },
                )?;
                write_payload(&mut file, &item, format)?;
                file.flush().map_err(Failure::from)
            }
            (Dest::Stream(stream), Format::Json) => {
                let _ = stream;
                self.buffered.push(item.value);
                Ok(())
            }
            (Dest::Stream(stream), format) => {
                write_payload(stream, &item, format)?;
                // Flushed per item on purpose. A caller reading the pipe is
                // meant to have this page before the next one is fetched.
                stream.flush().map_err(Failure::from)
            }
        }
    }

    /// Write the one thing this command produces.
    ///
    /// For a command whose answer is a document rather than a run of results,
    /// `schema` being the one. Under JSON it is written bare, with no wrapper
    /// around it, because a caller reading a contract should not have to reach
    /// through a list of one to find it.
    pub fn write_sole(&mut self, item: Item) -> Run<()> {
        self.sole = true;
        self.write(item)
    }

    /// Write the closing report.
    ///
    /// Under NDJSON it is the last line, which is where a caller reading a
    /// stream expects a summary. Under JSON it becomes a field of the one
    /// document, so `items` holds results and only results.
    pub fn write_report(&mut self, item: Item) -> Run<()> {
        if self.format == Format::Json {
            self.report = Some(item.value);
            return Ok(());
        }
        self.write(item)
    }

    /// Close the destination.
    ///
    /// JSON is written here, because one document cannot be written until the
    /// run that fills it has ended. Everything else has already left.
    pub fn finish(&mut self) -> Run<()> {
        if let (Dest::Stream(stream), Format::Json) = (&mut self.dest, self.format) {
            let document = if self.sole && self.buffered.len() == 1 {
                self.buffered.remove(0)
            } else {
                let mut document = serde_json::Map::new();
                document.insert(
                    "items".to_string(),
                    Value::Array(std::mem::take(&mut self.buffered)),
                );
                if let Some(report) = self.report.take() {
                    document.insert("report".to_string(), report);
                }
                Value::Object(document)
            };
            let encoded = serde_json::to_string(&document)
                .map_err(|e| Failure::new(Code::Failed, format!("could not encode: {e}")))?;
            stream.write_all(encoded.as_bytes())?;
            stream.write_all(b"\n")?;
        }
        if let Dest::Stream(stream) = &mut self.dest {
            stream.flush()?;
        }
        Ok(())
    }
}

/// Write one item to one stream, in the shape asked for.
fn write_payload(out: &mut dyn Write, item: &Item, format: Format) -> Run<()> {
    match format {
        Format::Text => match (&item.bytes, &item.text) {
            (Some(bytes), _) => out.write_all(bytes).map_err(Failure::from),
            (None, Some(text)) => {
                out.write_all(text.as_bytes())?;
                if !text.ends_with('\n') {
                    out.write_all(b"\n")?;
                }
                Ok(())
            }
            // Nothing to print is not a failure. A metadata request returns no
            // body on purpose.
            (None, None) => Ok(()),
        },
        Format::Json | Format::Ndjson => {
            let encoded = serde_json::to_string(&item.value)
                .map_err(|e| Failure::new(Code::Failed, format!("could not encode: {e}")))?;
            out.write_all(encoded.as_bytes())?;
            out.write_all(b"\n").map_err(Failure::from)
        }
    }
}

/// Open a file under the rules the caller gave.
fn open_file(path: &Path, rules: FileRules) -> Run<File> {
    if rules.mkdir {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    Failure::output(format!("could not create {}: {e}", parent.display()))
                })?;
            }
        }
    }
    if path.exists() && !rules.force && !rules.append {
        return Err(Failure::output(format!(
            "{} is already there. Pass --force to replace it, or --append to add to it.",
            path.display()
        )));
    }
    let mut options = OpenOptions::new();
    options.write(true).create(true);
    if rules.append {
        options.append(true);
    } else {
        options.truncate(true);
    }
    options
        .open(path)
        .map_err(|e| Failure::output(format!("could not open {}: {e}", path.display())))
}

/// The name one item gets inside a directory.
///
/// The same address gives the same name on every run, so two runs over one
/// list of addresses can be compared file by file. The short hash is there
/// because two addresses can flatten to the same readable part, and losing one
/// of them silently is worse than a name with eight hex digits on the end.
pub fn file_name(url: Option<&Url>, ordinal: usize, format: Format, extension: &str) -> String {
    let ext = match format {
        Format::Json | Format::Ndjson => "json",
        Format::Text => extension,
    };
    match url {
        Some(url) => format!("{}-{}.{ext}", slug(url), short_hash(url.as_str())),
        None => format!("item-{ordinal:06}.{ext}"),
    }
}

/// The readable part of a file name: the host and path, flattened.
pub fn slug(url: &Url) -> String {
    let mut out = String::new();
    let host = url.host_str().unwrap_or("address");
    push_safe(&mut out, host);
    let path = url.path();
    if path != "/" {
        push_safe(&mut out, path);
    }
    if let Some(query) = url.query() {
        push_safe(&mut out, query);
    }
    let trimmed: String = out.trim_matches('-').chars().take(80).collect();
    let trimmed = trimmed.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "address".to_string()
    } else {
        trimmed
    }
}

/// Append text with everything outside `a-z0-9-` turned into a single dash.
fn push_safe(out: &mut String, text: &str) {
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
}

/// Eight hex digits of FNV-1a over the whole address.
///
/// Not a checksum anyone verifies. It is there to keep two addresses that
/// flatten to the same readable name in two different files.
pub fn short_hash(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:08x}", (hash >> 32) as u32)
}

#[cfg(test)]
mod tests {
    // A test may unwrap and may panic: a test that cannot set itself up should
    // fail loudly rather than quietly measure nothing.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).expect("a url")
    }

    #[test]
    fn a_name_is_the_same_on_every_run() {
        let one = file_name(Some(&url("https://example.com/a/b")), 1, Format::Text, "md");
        let two = file_name(Some(&url("https://example.com/a/b")), 9, Format::Text, "md");
        assert_eq!(one, two);
        assert_eq!(one, "example-com-a-b-36964733.md");
    }

    #[test]
    fn a_name_carries_no_separator() {
        let name = file_name(
            Some(&url("https://example.com/../../etc/passwd")),
            1,
            Format::Json,
            "json",
        );
        assert!(!name.contains('/'), "{name}");
        assert!(!name.contains(".."), "{name}");
        assert!(name.ends_with(".json"), "{name}");
    }

    #[test]
    fn two_addresses_that_flatten_alike_keep_two_files() {
        let one = file_name(
            Some(&url("https://example.com/a?b=1")),
            1,
            Format::Text,
            "md",
        );
        let two = file_name(
            Some(&url("https://example.com/a-b-1")),
            1,
            Format::Text,
            "md",
        );
        assert_ne!(one, two, "the hash is what keeps these apart");
    }

    #[test]
    fn the_extension_follows_the_format() {
        let json = file_name(Some(&url("https://example.com/")), 1, Format::Ndjson, "md");
        assert!(json.ends_with(".json"), "{json}");
    }

    #[test]
    fn an_item_with_no_address_is_numbered() {
        assert_eq!(
            file_name(None, 7, Format::Text, "txt"),
            "item-000007.txt".to_string()
        );
    }
}
