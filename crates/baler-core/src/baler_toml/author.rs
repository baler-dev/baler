use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum Author {
    Simple(String),
    Extended {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        email: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        orcid: Option<String>,
    },
}

impl Author {
    pub fn name(&self) -> &str {
        match self {
            Author::Simple(n) => n,
            Author::Extended { name, .. } => name,
        }
    }

    pub fn email(&self) -> Option<&str> {
        match self {
            Author::Simple(_) => None,
            Author::Extended { email, .. } => email.as_deref(),
        }
    }

    pub fn url(&self) -> Option<&str> {
        match self {
            Author::Simple(_) => None,
            Author::Extended { url, .. } => url.as_deref(),
        }
    }

    pub fn orcid(&self) -> Option<&str> {
        match self {
            Author::Simple(_) => None,
            Author::Extended { orcid, .. } => orcid.as_deref(),
        }
    }
}

impl std::fmt::Display for Author {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Author::Simple(n) => write!(f, "{}", n),
            Author::Extended { name, email, url, .. } => {
                write!(f, "{}", name)?;
                if let Some(e) = email { write!(f, " <{}>", e)?; }
                if let Some(u) = url { write!(f, " ({})", u)?; }
                Ok(())
            }
        }
    }
}
