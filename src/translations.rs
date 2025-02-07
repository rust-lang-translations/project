use crate::build::build_book;
use crate::serve::serve;
use anyhow::{bail, Result};
use log::info;
use mdbook::MDBook;
use mdbook_i18n_helpers::renderers::Xgettext;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Translations {
    pub submodules: Vec<Vec<PathBuf>>,
    pub books: BTreeMap<String, Book>,
    #[serde(skip)]
    base: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Book {
    pub path: PathBuf,
    pub translations: Vec<Translation>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Translation {
    pub id: String,
    pub name: String,
}

#[allow(unused)]
#[derive(Clone, Debug)]
pub struct TranslationStat {
    pub name: String,
    pub title: String,
    pub langs: HashMap<String, TranslationLangStat>,
}

#[allow(unused)]
#[derive(Clone, Debug)]
pub struct TranslationLangStat {
    pub lang_name: String,
    pub total_text: usize,
    pub translated_text: usize,
    pub translation_ratio: f32,
}

impl Translations {
    pub fn load() -> Result<Self> {
        let base = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let base = PathBuf::from(base);

        let toml = base.join("translations.toml");
        let toml = std::fs::read_to_string(toml)?;
        let mut toml: Translations = toml::from_str(&toml)?;
        toml.base = base;

        Ok(toml)
    }

    pub fn save(&self) -> Result<()> {
        let mut text = String::new();
        text.push_str(&toml::to_string_pretty(&self)?);

        let toml = self.base.join("translations.toml");
        std::fs::write(&toml, text.as_bytes())?;
        Ok(())
    }

    pub fn build(&self) -> Result<()> {
        self.update_submodule()?;

        for (name, book) in &self.books {
            let src_path = self.src_path(&book.path);
            let dst_path = self.dst_path(name);
            let po_path = self.po_path(name);

            build_book(name, &book, &src_path, &dst_path, &po_path, None)?;
        }
        Ok(())
    }

    pub fn add(&mut self, name: &str, lang_id: &str, lang_name: &str) -> Result<()> {
        if let Some(book) = self.books.get(name) {
            let src_path = self.src_path(&book.path);
            let po_path = self.po_path(name);

            self.update_submodule()?;
            extract_pot(&src_path, &po_path)?;

            let lang_po = po_path.join(format!("{lang_id}.po"));

            if lang_po.exists() {
                bail!("Language {lang_id} for {name} alreay exists");
            }

            Command::new("msginit")
                .arg("--no-translator")
                .arg("-i")
                .arg(po_path.join("messages.pot"))
                .arg("-l")
                .arg(lang_id)
                .arg("-o")
                .arg(&lang_po)
                .output()?;

            let new_trans = Translation {
                id: lang_id.to_string(),
                name: lang_name.to_string(),
            };

            self.books
                .entry(name.to_string())
                .and_modify(|x| x.translations.push(new_trans));

            self.save()?;
        }

        Ok(())
    }

    pub fn update(&self, name: &str, lang_id: &str) -> Result<()> {
        if let Some(book) = self.books.get(name) {
            let src_path = self.src_path(&book.path);
            let po_path = self.po_path(name);

            self.update_submodule()?;
            extract_pot(&src_path, &po_path)?;

            let lang_po = po_path.join(format!("{lang_id}.po"));

            if !lang_po.exists() {
                bail!("Language {lang_id} for {name} is not found");
            }

            Command::new("msgmerge")
                .arg("--update")
                .arg(&lang_po)
                .arg(po_path.join("messages.pot"))
                .output()?;
        }

        Ok(())
    }

    pub fn serve(&self, name: &str, lang_id: &str, hostname: &str, port: &str) -> Result<()> {
        if let Some(book) = self.books.get(name) {
            let src_path = self.src_path(&book.path);
            let dst_path = self.dst_path(name);
            let po_path = self.po_path(name);

            serve(
                name, book, &src_path, &dst_path, &po_path, lang_id, hostname, port,
            )?;
        }

        Ok(())
    }

    pub fn stat(&self) -> Result<Vec<TranslationStat>> {
        let mut ret = Vec::new();

        for (name, book) in &self.books {
            let src_path = self.src_path(&book.path);
            let po_path = self.po_path(name);
            let mdbook = MDBook::load(&src_path)?;
            let title = mdbook.config.book.title.unwrap_or("".to_string());

            let mut stat = TranslationStat {
                name: name.clone(),
                title: title.clone(),
                langs: HashMap::new(),
            };

            for lang in &book.translations {
                let lang_po = po_path.join(format!("{}.po", lang.id));
                let catalog = polib::po_file::parse(&lang_po)?;
                let total_text = catalog.count();
                let translated_text = catalog.messages().filter(|x| x.is_translated()).count();
                let translation_ratio = translated_text as f32 / total_text as f32;

                let lang_stat = TranslationLangStat {
                    lang_name: lang.name.clone(),
                    total_text,
                    translated_text,
                    translation_ratio,
                };
                stat.langs.insert(lang.id.clone(), lang_stat);
            }
            ret.push(stat);
        }

        Ok(ret)
    }

    fn src_path(&self, path: &Path) -> PathBuf {
        self.base.join(path)
    }

    fn dst_path(&self, name: &str) -> PathBuf {
        self.base.join("build").join(name)
    }

    fn po_path(&self, name: &str) -> PathBuf {
        self.base.join("translations").join(name)
    }

    fn update_submodule(&self) -> Result<()> {
        for submodule in &self.submodules {
            let mut base: Option<PathBuf> = None;
            for path in submodule {
                if let Some(base) = &base {
                    info!(
                        "Update submodule {} from {}",
                        path.to_string_lossy(),
                        base.to_string_lossy()
                    );
                    Command::new("git")
                        .arg("-C")
                        .arg(base)
                        .arg("submodule")
                        .arg("update")
                        .arg("--init")
                        .arg(path)
                        .output()?;
                } else {
                    info!("Update submodule {}", path.to_string_lossy());
                    Command::new("git")
                        .arg("submodule")
                        .arg("update")
                        .arg("--init")
                        .arg(path)
                        .output()?;
                }
                base = Some(base.map_or(path.clone(), |x: PathBuf| x.join(path)));
            }
        }

        Ok(())
    }
}

fn extract_pot(src_path: &Path, po_path: &Path) -> Result<()> {
    let mut mdbook = MDBook::load(&src_path)?;
    mdbook.config.build.build_dir = PathBuf::from(po_path);
    let renderer = Xgettext {};
    mdbook.clear_renderers();
    mdbook.with_renderer(renderer);
    mdbook.build()?;

    Ok(())
}
