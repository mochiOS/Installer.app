//! Runtime UTF-8 documents, separately packaged for post-install viewing.
use appcore::consent::{ConsentRecord, DataCategory, DocumentReceipt};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct License {
    pub name: String,
    pub version: Option<String>,
    pub license: String,
    pub copyright_file: String,
    pub body_file: String,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisclosureField {
    pub key: String,
    pub title: String,
    pub file: String,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Disclosure {
    pub summary_file: String,
    pub fields: Vec<DisclosureField>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataPolicy {
    pub category: DataCategory,
    pub version: String,
    pub text: BTreeMap<String, Disclosure>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub schema_version: u32,
    pub document_version: String,
    pub licenses: Vec<License>,
    pub data_policies: Vec<DataPolicy>,
}
#[derive(Clone, Debug)]
pub struct Documents {
    pub catalog: Catalog,
    pub files: BTreeMap<String, String>,
}
impl Documents {
    pub fn load(root: &Path) -> io::Result<Self> {
        let catalog: Catalog =
            serde_json::from_str(&read(root, "catalog.json")?).map_err(io::Error::other)?;
        if catalog.schema_version != 1
            || catalog.document_version.is_empty()
            || catalog.licenses.is_empty()
            || catalog.licenses.len() > 256
        {
            return Err(io::Error::other("Unsupported legal document catalog"));
        }
        let mut files = BTreeMap::new();
        for name in ["terms.txt", "privacy.txt"] {
            insert(&mut files, root, name)?;
        }
        for license in &catalog.licenses {
            for name in [&license.copyright_file, &license.body_file] {
                insert(&mut files, root, name)?;
            }
        }
        if catalog.data_policies.len() != 3 {
            return Err(io::Error::other(
                "Each data category requires its own policy",
            ));
        }
        let mut categories = std::collections::BTreeSet::new();
        for policy in &catalog.data_policies {
            if !categories.insert(policy.category.index())
                || policy.version.is_empty()
                || policy.text.len() > 8
                || !policy.text.contains_key("en")
            {
                return Err(io::Error::other("Invalid data policy catalog"));
            }
            for disclosure in policy.text.values() {
                insert(&mut files, root, &disclosure.summary_file)?;
                let required: std::collections::BTreeSet<_> = [
                    "purpose",
                    "data",
                    "destination",
                    "retention",
                    "exclusions",
                    "settings",
                ]
                .into_iter()
                .collect();
                let keys: std::collections::BTreeSet<_> = disclosure
                    .fields
                    .iter()
                    .map(|field| field.key.as_str())
                    .collect();
                if disclosure.fields.len() != 6 || keys != required {
                    return Err(io::Error::other("Incomplete data policy disclosure"));
                }
                for field in &disclosure.fields {
                    insert(&mut files, root, &field.file)?;
                }
            }
        }
        Ok(Self { catalog, files })
    }
    pub fn policy(&self, category: DataCategory) -> &DataPolicy {
        self.catalog
            .data_policies
            .iter()
            .find(|p| p.category == category)
            .expect("validated policy catalog")
    }
    pub fn disclosure(&self, category: DataCategory, locale: &str) -> &Disclosure {
        let policy = self.policy(category);
        policy.text.get(locale).unwrap_or(&policy.text["en"])
    }
    pub fn text(&self, name: &str) -> &str {
        self.files.get(name).map_or("", String::as_str)
    }
    /// Bind the OSS receipt to the catalog and exact copyright/body bytes.
    pub fn license_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        for license in &self.catalog.licenses {
            for text in [
                license.name.as_str(),
                license.version.as_deref().unwrap_or(""),
                license.license.as_str(),
                self.text(&license.copyright_file),
                self.text(&license.body_file),
            ] {
                bytes.extend_from_slice(&(text.len() as u64).to_le_bytes());
                bytes.extend_from_slice(text.as_bytes());
            }
        }
        bytes
    }
    pub fn receipt(&self, name: &str, at: u64) -> DocumentReceipt {
        DocumentReceipt::new(
            &self.catalog.document_version,
            self.text(name).as_bytes(),
            at,
        )
    }
    pub fn legal_matches(&self, record: &ConsentRecord) -> bool {
        record.terms.as_ref().is_some_and(|r| {
            r.matches(
                &self.catalog.document_version,
                self.text("terms.txt").as_bytes(),
            )
        }) && record.privacy.as_ref().is_some_and(|r| {
            r.matches(
                &self.catalog.document_version,
                self.text("privacy.txt").as_bytes(),
            )
        }) && record
            .open_source_licenses
            .as_ref()
            .is_some_and(|r| r.matches(&self.catalog.document_version, &self.license_bytes()))
    }
}
fn insert(files: &mut BTreeMap<String, String>, root: &Path, name: &str) -> io::Result<()> {
    if !files.contains_key(name) {
        let body = read(root, name)?;
        if files.values().map(String::len).sum::<usize>() + body.len() > 16 * 1024 * 1024 {
            return Err(io::Error::other("Legal bundle exceeds its size limit"));
        }
        files.insert(name.into(), body);
    }
    Ok(())
}
fn read(root: &Path, name: &str) -> io::Result<String> {
    if Path::new(name).components().count() != 1 || name.starts_with('.') {
        return Err(io::Error::other("Invalid legal document path"));
    }
    let mut bytes = Vec::new();
    fs::File::open(root.join(name))?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        return Err(io::Error::other("Legal document is too large"));
    }
    String::from_utf8(bytes).map_err(io::Error::other)
}
pub fn resource_root() -> PathBuf {
    #[cfg(target_os = "mochios")]
    {
        PathBuf::from("/system/applications/Installer.app/legal")
    }
    #[cfg(not(target_os = "mochios"))]
    {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("legal")
    }
}
