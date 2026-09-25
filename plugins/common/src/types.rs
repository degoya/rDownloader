//! Types both adapters can build, owned by neither of them.
//!
//! Deliberately a copy of neither side's vocabulary: converting once at each adapter is the
//! price of writing the logic once, and it keeps this crate free of both `rd-core` and the
//! generated WIT bindings.

/// Why something failed, in the categories the scheduler acts on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FailureKind {
    /// Worth retrying; `Some` names how long to wait first.
    Transient(Option<u64>),
    Permanent,
    Offline,
    AuthRequired,
    AccountInvalid,
    RateLimited(Option<u64>),
    NeedsCaptcha,
    Unsupported,
    /// This IP may not start another free download from the hoster yet.
    IpBlocked(Option<u64>),
    /// A captcha was answered and the hoster rejected the answer.
    CaptchaFailed,
}

/// One failure, with the stable code the UI translates.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Failure {
    pub kind: FailureKind,
    /// English, redaction-safe text.
    pub message: String,
    /// Stable translation code such as `ddownload.captcha_rejected`.
    pub code: Option<String>,
    pub params: Vec<(String, String)>,
}

impl Failure {
    #[must_use]
    pub fn coded(kind: FailureKind, code: &str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            code: Some(code.to_owned()),
            params: Vec::new(),
        }
    }

    /// Adds one parameter the translated text can reference.
    #[must_use]
    pub fn with_param(mut self, name: &str, value: impl Into<String>) -> Self {
        self.params.push((name.to_owned(), value.into()));
        self
    }
}

/// A request header or query parameter. The value may carry a `{{secret:…}}` marker, which the
/// host expands; the value itself never enters the plugin.
#[derive(Clone, Debug)]
pub struct Header {
    pub name: String,
    pub value: String,
}

impl Header {
    #[must_use]
    pub fn new(name: &str, value: impl Into<String>) -> Self {
        Self {
            name: name.to_owned(),
            value: value.into(),
        }
    }
}

/// One outbound request, as the host will make it.
#[derive(Clone, Debug)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub query: Vec<Header>,
    pub headers: Vec<Header>,
    pub body: Vec<u8>,
}

impl HttpRequest {
    #[must_use]
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: "GET".to_owned(),
            url: url.into(),
            query: Vec::new(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    #[must_use]
    pub fn post(url: impl Into<String>, body: Vec<u8>) -> Self {
        Self {
            method: "POST".to_owned(),
            url: url.into(),
            query: Vec::new(),
            headers: Vec::new(),
            body,
        }
    }

    #[must_use]
    pub fn with_header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push(Header::new(name, value));
        self
    }

    #[must_use]
    pub fn with_query(mut self, name: &str, value: impl Into<String>) -> Self {
        self.query.push(Header::new(name, value));
        self
    }
}

/// The host's answer, already bounded and redirect-checked.
#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub final_url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// Looks up a response header case-insensitively.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The body as text, lossily; hoster pages are not always valid UTF-8.
    #[must_use]
    pub fn text(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }
}

/// A widget captcha, solvable from its site key and the page it sits on.
#[derive(Clone, Debug)]
pub struct WidgetChallenge {
    pub site_key: String,
    pub page_url: String,
    pub invisible: bool,
}

/// A classic image captcha.
#[derive(Clone, Debug)]
pub struct ImageChallenge {
    pub mime: String,
    pub data: Vec<u8>,
    pub prompt: Option<String>,
}

/// A CutCaptcha widget: its own identifier (`data-apikey`), the page's
/// `CUTCAPTCHA_MISERY_KEY`, and the page. Answered by a solver service only.
#[derive(Clone, Debug)]
pub struct CutcaptchaChallenge {
    pub site_key: String,
    pub misery_key: String,
    pub page_url: String,
}

/// What the host is asked to solve.
#[derive(Clone, Debug)]
pub enum CaptchaChallenge {
    RecaptchaV2(WidgetChallenge),
    HCaptcha(WidgetChallenge),
    Turnstile(WidgetChallenge),
    Image(ImageChallenge),
    /// A picture answered by clicking one spot in it; ask through `solve_challenge`.
    ClickPoint(ImageChallenge),
    Cutcaptcha(CutcaptchaChallenge),
}

/// A widget token, or the typed text of an image captcha.
#[derive(Clone, Debug)]
pub struct CaptchaSolution {
    pub token: String,
}

/// The spot clicked in a click-point captcha, in pixels of the image as served.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClickPoint {
    pub x: u32,
    pub y: u32,
}

/// The answer in the challenge's own shape.
#[derive(Clone, Debug)]
pub enum CaptchaAnswer {
    Token(String),
    Point(ClickPoint),
}

/// What a resolver is asked to turn into a download.
#[derive(Clone, Debug)]
pub struct ResolveInput {
    pub url: String,
    /// `None` is an account-less (free) resolve, which not every hoster offers.
    pub account_id: Option<String>,
}

/// The download a resolver produced.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub url: String,
    pub file_name: Option<String>,
    pub size: Option<u64>,
    /// Headers the transfer must repeat, such as the `Referer` a free link was earned with.
    pub headers: Vec<Header>,
    /// `(algorithm, value)` when the hoster states one.
    pub checksum: Option<(String, String)>,
}

impl Resolved {
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            file_name: None,
            size: None,
            headers: Vec::new(),
            checksum: None,
        }
    }
}

/// What a provider account is worth.
#[derive(Clone, Debug)]
pub struct Account {
    pub valid: bool,
    pub premium: bool,
    /// Shown in the account list as translated parts; never a credential. Build it with
    /// [`crate::Label`]; empty when the check has nothing to add to the two flags.
    pub label: Vec<crate::LabelPart>,
    /// Remaining traffic **in bytes**, whatever unit the provider's API happens to use.
    pub traffic_left: Option<u64>,
}

/// A batch of links to check.
#[derive(Clone, Debug)]
pub struct CheckInput {
    pub urls: Vec<String>,
    pub account_id: Option<String>,
}

/// Whether a link is still there.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkStatus {
    Online,
    Offline,
    Unknown,
    /// The provider holds the file in its own cache right now (RD-120-36). Only for a file
    /// the provider said is cached; one it knows but has not fetched is `Online`.
    Cached,
}

/// One link's answer.
#[derive(Clone, Debug)]
pub struct LinkCheck {
    pub url: String,
    pub status: LinkStatus,
    pub file_name: Option<String>,
    pub size: Option<u64>,
}
