use std::time::Instant;

pub struct Notification {
    pub text: String,
    created: Instant,
    expires: Option<u64>,
}

impl Notification {
    pub fn new(text: String) -> Self {
        Self {
            text,
            created: Instant::now(),
            expires: None,
        }
    }

    pub fn expires(mut self, expires: u64) -> Self {
        self.expires = Some(expires);
        self
    }

    pub fn expired(&self) -> bool {
        match self.expires {
            None => false,
            Some(expires) => (Instant::now() - self.created).as_secs() > expires,
        }
    }
}
