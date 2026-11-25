use secrecy::SecretString;

pub struct Credentials {
    url: String,
    method: Method,
}

pub enum Method {
    Token(SecretString),
    Agent {
        user: String,
        host: String,
        key: Vec<u8>,
    },
}

impl Credentials {
    #[must_use]
    pub const fn https(url: String, token: SecretString) -> Self {
        Self {
            url,
            method: Method::Token(token),
        }
    }

    #[must_use]
    pub const fn ssh(
        url: String,
        user: String,
        host: String,
        key: Vec<u8>,
    ) -> Self {
        Self {
            url,
            method: Method::Agent { user, host, key },
        }
    }

    #[must_use]
    pub const fn method(&self) -> &Method {
        &self.method
    }

    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }
}
