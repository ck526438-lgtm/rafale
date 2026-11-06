use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    Go,
    Python,
    JavaScript,
    TypeScript,
}

pub const EXTENSIONS: &[(&str, Language)] = &[
    ("rs", Language::Rust),
    ("go", Language::Go),
    ("py", Language::Python),
    ("js", Language::JavaScript),
    ("mjs", Language::JavaScript),
    ("cjs", Language::JavaScript),
    ("jsx", Language::JavaScript),
    ("ts", Language::TypeScript),
    ("mts", Language::TypeScript),
    ("tsx", Language::TypeScript),
];

impl Language {
    pub fn from_path(path: &str) -> Option<Language> {
        let ext = path.rsplit('.').next().unwrap_or("");
        EXTENSIONS.iter().find(|(e, _)| *e == ext).map(|(_, l)| *l)
    }

    pub fn uses_indent_blocks(self) -> bool {
        matches!(self, Language::Python)
    }

    pub fn line_comment(self) -> &'static str {
        match self {
            Language::Python => "#",
            _ => "//",
        }
    }

    pub fn block_comment(self) -> Option<(&'static str, &'static str)> {
        match self {
            Language::Python => None,
            Language::Rust | Language::Go | Language::JavaScript | Language::TypeScript => {
                Some(("/*", "*/"))
            }
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Language::Rust => "rust",
            Language::Go => "go",
            Language::Python => "python",
            Language::JavaScript => "javascript",
            Language::TypeScript => "typescript",
        };
        f.write_str(s)
    }
}
