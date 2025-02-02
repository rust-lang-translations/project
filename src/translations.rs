use anyhow::{bail, Result};
use mdbook::MDBook;
use mdbook_i18n_helpers::preprocessors::Gettext;
use mdbook_i18n_helpers::renderers::Xgettext;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use tera::Tera;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Translations {
    pub books: BTreeMap<String, Book>,
    #[serde(skip)]
    base: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Book {
    pub translations: Vec<Translation>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Translation {
    pub id: String,
    pub name: String,
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
        text.push_str(&toml::to_string(&self)?);

        let toml = self.base.join("translations.toml");
        std::fs::write(&toml, text.as_bytes())?;
        Ok(())
    }

    pub fn build(&self) -> Result<()> {
        update_submodule()?;

        for (name, book) in &self.books {
            let src_path = self.src_path(name);
            let dst_path = PathBuf::from(format!("../../build/{}", name));
            let po_path = self.po_path(name);

            build_book(&book, &src_path, &dst_path, &po_path)?;
        }
        Ok(())
    }

    pub fn add(&mut self, book: &str, lang_id: &str, lang_name: &str) -> Result<()> {
        let src_path = self.src_path(book);
        let po_path = self.po_path(book);

        update_submodule()?;
        extract_pot(&src_path, &po_path)?;

        let lang_po = po_path.join(format!("{lang_id}.po"));

        if lang_po.exists() {
            bail!("Language {lang_id} for {book} alreay exists");
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

        let new_book = Book {
            translations: vec![new_trans.clone()],
        };

        self.books
            .entry(book.to_string())
            .and_modify(|x| x.translations.push(new_trans))
            .or_insert(new_book);

        self.save()?;

        Ok(())
    }

    pub fn update(&self, book: &str, lang_id: &str) -> Result<()> {
        let src_path = self.src_path(book);
        let po_path = self.po_path(book);

        update_submodule()?;
        extract_pot(&src_path, &po_path)?;

        let lang_po = po_path.join(format!("{lang_id}.po"));

        if !lang_po.exists() {
            bail!("Language {lang_id} for {book} is not found");
        }

        Command::new("msgmerge")
            .arg("--update")
            .arg(&lang_po)
            .arg(po_path.join("messages.pot"))
            .output()?;

        Ok(())
    }

    fn src_path(&self, name: &str) -> PathBuf {
        self.base.join("repos").join(name)
    }

    fn po_path(&self, name: &str) -> PathBuf {
        self.base.join("translations").join(name)
    }
}

fn update_submodule() -> Result<()> {
    Command::new("git")
        .arg("submodule")
        .arg("update")
        .arg("--init")
        .arg("--recursive")
        .output()?;

    Ok(())
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

fn build_book(book: &Book, src_path: &Path, dst_path: &Path, po_path: &Path) -> Result<()> {
    let mut mdbook = MDBook::load(&src_path)?;
    mdbook.config.build.build_dir = PathBuf::from(dst_path);

    let mut tmpl = Tera::default();
    tmpl.add_raw_template("lang-picker-js", LANG_PICKER_JS)?;

    let mut context = tera::Context::new();
    context.insert("langs", &book.translations);
    let lang_picker_js = tmpl.render("lang-picker-js", &context)?;

    let theme_dir = mdbook.theme_dir();
    if !theme_dir.exists() {
        std::fs::create_dir(&theme_dir)?;
    }
    let js_path = theme_dir.join("language-picker.js");
    let css_path = theme_dir.join("language-picker.css");

    std::fs::write(&js_path, lang_picker_js)?;
    std::fs::write(&css_path, LANG_PICKER_CSS)?;

    let js_file: toml::Value = "theme/language-picker.js".into();
    let css_file: toml::Value = "theme/language-picker.css".into();
    let po_path: toml::Value = po_path.to_string_lossy().into_owned().into();

    if let Some(additional_css) = mdbook.config.get_mut("output.html.additional-css") {
        additional_css.as_array_mut().unwrap().push(css_file.into());
    } else {
        mdbook
            .config
            .set("output.html.additional-css", vec![css_file])?;
    }
    if let Some(additional_js) = mdbook.config.get_mut("output.html.additional-js") {
        additional_js.as_array_mut().unwrap().push(js_file.into());
    } else {
        mdbook
            .config
            .set("output.html.additional-js", vec![js_file])?;
    }
    mdbook.config.set("preprocessor.gettext.po-dir", po_path)?;

    mdbook.build()?;

    let gettext = Gettext;
    mdbook.with_preprocessor(gettext);
    for lang in &book.translations {
        mdbook.config.build.build_dir = dst_path.join(&lang.id);
        mdbook.config.set("book.language", &lang.id)?;
        mdbook.build()?;
    }

    std::fs::remove_file(&js_path)?;
    std::fs::remove_file(&css_path)?;

    Ok(())
}

const LANG_PICKER_JS: &str = r#"
const right_buttons = document.getElementsByClassName('right-buttons')[0];
const language_toggle = `
<button id="language-toggle" class="icon-button" type="button"
        title="Change language" aria-label="Change language"
        aria-haspopup="true" aria-expanded="false"
        aria-controls="language-list">
    <i class="fa fa-globe"></i>
</button>
<ul id="language-list" class="theme-popup" aria-label="Languages" role="menu">
  <li role="none"><button role="menuitem" class="theme">
      <a id="en">English</a>
  </button></li>
  {% for lang in langs %}
  <li role="none"><button role="menuitem" class="theme">
      <a id="{{ lang.id }}">{{ lang.name }}</a>
  </button></li>
  {% endfor %}
</ul>
`;
right_buttons.insertAdjacentHTML('afterbegin', language_toggle);

let langToggle = document.getElementById("language-toggle");
let langList = document.getElementById("language-list");
langToggle.addEventListener("click", (event) => {
    langList.style.display = langList.style.display == "block" ? "none" : "block";
});
let lang = document.documentElement.lang;
let selectedLang = document.getElementById(lang);
if (selectedLang) {
    selectedLang.parentNode.classList.add("theme-selected");
}

// The path to the root, taking the current
// language into account.
var full_path_to_root;
if (lang == "en") {
    full_path_to_root = `${path_to_root}`;
} else {
    full_path_to_root = `${path_to_root}../`;
}
let components = (path_to_root.match(/\//g) ?? []).length + 1;
let path = window.location.pathname.split('/').slice(-components).join('/');
for (let lang of langList.querySelectorAll("a")) {
    if (lang.id == "en") {
        lang.href = `${full_path_to_root}${path}`;
    } else {
        lang.href = `${full_path_to_root}${lang.id}/${path}`;
    }
}
"#;

const LANG_PICKER_CSS: &str = r#"
#language-list {
  left: auto;
  right: 10px;
}

[dir="rtl"] #language-list {
  left: 10px;
  right: auto;
}

#language-list a {
  color: inherit;
}
"#;
