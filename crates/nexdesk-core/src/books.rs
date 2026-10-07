//! Address books: named groups of saved connections (a group of saved connections).
//! Stored in `~/.config/nexdesk/addressbooks` as `book<TAB>connection` lines; a line with an
//! empty connection keeps an empty book alive. Connections are referenced by profile name.
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Book {
    pub name: String,
    pub members: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AddressBooks {
    pub books: Vec<Book>,
    path: Option<PathBuf>,
}

pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("nexdesk").join("addressbooks"))
}

fn clean(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string()
}

impl AddressBooks {
    pub fn in_memory() -> Self {
        Self::default()
    }

    pub fn load(path: &Path) -> Self {
        let mut me = Self { books: Vec::new(), path: Some(path.to_path_buf()) };
        if let Ok(text) = std::fs::read_to_string(path) {
            for line in text.lines() {
                let mut p = line.splitn(2, '\t');
                let book = clean(p.next().unwrap_or(""));
                if book.is_empty() {
                    continue;
                }
                let _ = me.create(&book);
                if let Some(m) = p.next().map(clean).filter(|m| !m.is_empty()) {
                    let _ = me.add(&book, &m);
                }
            }
        }
        me
    }

    fn save(&self) -> io::Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut s = String::new();
        for b in &self.books {
            if b.members.is_empty() {
                s.push_str(&format!("{}\t\n", b.name));
            }
            for m in &b.members {
                s.push_str(&format!("{}\t{}\n", b.name, m));
            }
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, s)?;
        std::fs::rename(tmp, path)
    }

    pub fn create(&mut self, name: &str) -> Result<(), &'static str> {
        let name = clean(name);
        if name.is_empty() {
            return Err("Name is empty");
        }
        if self.books.iter().any(|b| b.name.eq_ignore_ascii_case(&name)) {
            return Err("An address book with this name already exists");
        }
        self.books.push(Book { name, members: Vec::new() });
        self.save().map_err(|_| "Could not save address books")
    }

    pub fn delete(&mut self, name: &str) {
        self.books.retain(|b| b.name != name);
        let _ = self.save();
    }

    pub fn add(&mut self, book: &str, connection: &str) -> Result<(), &'static str> {
        let b = self.books.iter_mut().find(|b| b.name == book).ok_or("No such address book")?;
        if !b.members.iter().any(|m| m == connection) {
            b.members.push(connection.to_string());
        }
        self.save().map_err(|_| "Could not save address books")
    }

    pub fn remove(&mut self, book: &str, connection: &str) {
        if let Some(b) = self.books.iter_mut().find(|b| b.name == book) {
            b.members.retain(|m| m != connection);
        }
        let _ = self.save();
    }

    /// Keep references valid when a connection is renamed or deleted (`new` = None).
    pub fn rename_connection(&mut self, old: &str, new: Option<&str>) {
        for b in &mut self.books {
            if let Some(i) = b.members.iter().position(|m| m == old) {
                match new {
                    Some(n) => b.members[i] = n.to_string(),
                    None => {
                        b.members.remove(i);
                    }
                }
            }
        }
        let _ = self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_add_persist() {
        let p = std::env::temp_dir().join(format!("nexdesk-books-{}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let mut a = AddressBooks::load(&p);
        a.create("Production").unwrap();
        a.create("Empty").unwrap();
        assert!(a.create("production").is_err());
        a.add("Production", "Bank DB").unwrap();
        a.add("Production", "Bank DB").unwrap(); // idempotent
        let b = AddressBooks::load(&p);
        assert_eq!(b.books.len(), 2);
        assert_eq!(b.books[0].members, vec!["Bank DB"]);
        let mut b = b;
        b.rename_connection("Bank DB", Some("Bank DB 2"));
        b.remove("Production", "Bank DB 2");
        b.delete("Empty");
        let c = AddressBooks::load(&p);
        assert_eq!(c.books.len(), 1);
        assert!(c.books[0].members.is_empty());
        let _ = std::fs::remove_file(&p);
    }
}
