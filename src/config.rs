use std::process::exit;
use std::sync::RwLock;

use crate::error::Error;
use crate::util::{get_env, get_env_bool};

lazy_static! {
    pub static ref CONFIG: Config = Config::load().unwrap_or_else(|e| {
        println!("Error loading config:\n\t{:?}\n", e);
        exit(12)
    });
    pub static ref CONFIG_FILE: String = {
        let data_folder = get_env("DATA_FOLDER").unwrap_or_else(|| String::from("data"));
        get_env("CONFIG_FILE").unwrap_or_else(|| format!("{}/config.json", data_folder))
    };
}

pub type Pass = String;

macro_rules! make_config {
    ($(
        $(#[doc = $groupdoc:literal])?
        $group:ident $(: $group_enabled:ident)? {
        $(
            $(#[doc = $doc:literal])+
            $name:ident : $ty:ident, $editable:literal, $none_action:ident $(, $default:expr)?;
        )+},
    )+) => {
        pub struct Config { inner: RwLock<Inner> }

        struct Inner {
            rocket_shutdown_handle: Option<rocket::shutdown::ShutdownHandle>,
            ws_shutdown_handle: Option<ws::Sender>,

            templates: Handlebars<'static>,
            config: ConfigItems,

            _env: ConfigBuilder,
            _usr: ConfigBuilder,
        }

        #[derive(Debug, Clone, Default, Deserialize, Serialize)]
        pub struct ConfigBuilder {
            $($(
                #[serde(skip_serializing_if = "Option::is_none")]
                $name: Option<$ty>,
            )+)+
        }

        impl ConfigBuilder {
            fn from_env() -> Self {
                dotenv::from_path(".env").ok();

                let mut builder = ConfigBuilder::default();
                $($(
                    builder.$name = make_config! { @getenv &stringify!($name).to_uppercase(), $ty };
                )+)+
                
                builder
            }

            fn from_file(path: &str) -> Result<Self, Error> {
                use crate::util::read_file_string;
                let config_str = read_file_string(path)?;
                serde_json::from_str(&config_str).map_err(Into::into)
            }

            /// Merges the values of both builders into a new builder.
            /// If both have the same element, `other` wins.
            fn merge(&self, other: &Self, show_overrides: bool) -> Self {
                let mut overrides = Vec::new();
                let mut builder = self.clone();
                $($(
                    if let v @Some(_) = &other.$name {
                        builder.$name = v.clone();

                        if self.$name.is_some() {
                            overrides.push(stringify!($name).to_uppercase());
                        }
                    }
                )+)+

                if show_overrides && !overrides.is_empty() {
                    // We can't use warn! here because logging isn't setup yet.
                    println!("[WARNING] The following environment variables are being overriden by the config file,");
                    println!("[WARNING] please use the admin panel to make changes to them:");
                    println!("[WARNING] {}\n", overrides.join(", "));
                }

                builder
            }

            /// Returns a new builder with all the elements from self,
            /// except those that are equal in both sides
            fn _remove(&self, other: &Self) -> Self {
                let mut builder = ConfigBuilder::default();
                $($(
                    if &self.$name != &other.$name {
                        builder.$name = self.$name.clone();
                    }

                )+)+
                builder
            }

            fn build(&self) -> ConfigItems {
                let mut config = ConfigItems::default();
                let _domain_set = self.domain.is_some();
                $($(
                    config.$name = make_config!{ @build self.$name.clone(), &config, $none_action, $($default)? };
                )+)+
                config.domain_set = _domain_set;

                config
            }
        }

        #[derive(Debug, Clone, Default)]
        pub struct ConfigItems { $($(pub $name: make_config!{@type $ty, $none_action}, )+)+ }

        #[allow(unused)]
        impl Config {
            $($(
                pub fn $name(&self) -> make_config!{@type $ty, $none_action} {
                    self.inner.read().unwrap().config.$name.clone()
                }
            )+)+

            pub fn prepare_json(&self) -> serde_json::Value {
                let (def, cfg) = {
                    let inner = &self.inner.read().unwrap();
                    (inner._env.build(), inner.config.clone())
                };


                fn _get_form_type(rust_type: &str) -> &'static str {
                    match rust_type {
                        "Pass" => "password",
                        "String" => "text",
                        "bool" => "checkbox",
                        _ => "number"
                    }
                }

                fn _get_doc(doc: &str) -> serde_json::Value {
                    let mut split = doc.split("|>").map(str::trim);
                    json!({
                        "name": split.next(),
                        "description": split.next()
                    })
                }

                json!([ $({
                    "group": stringify!($group),
                    "grouptoggle": stringify!($($group_enabled)?),
                    "groupdoc": make_config!{ @show $($groupdoc)? },
                    "elements": [
                    $( {
                        "editable": $editable,
                        "name": stringify!($name),
                        "value": cfg.$name,
                        "default": def.$name,
                        "type":  _get_form_type(stringify!($ty)),
                        "doc": _get_doc(concat!($($doc),+)),
                    }, )+
                    ]}, )+ ])
            }
        }
    };

    // Group or empty string
    ( @show ) => { "" };
    ( @show $lit:literal ) => { $lit };

    // Wrap the optionals in an Option type
    ( @type $ty:ty, option) => { Option<$ty> };
    ( @type $ty:ty, $id:ident) => { $ty };

    // Generate the values depending on none_action
    ( @build $value:expr, $config:expr, option, ) => { $value };
    ( @build $value:expr, $config:expr, def, $default:expr ) => { $value.unwrap_or($default) };
    ( @build $value:expr, $config:expr, auto, $default_fn:expr ) => {{
        match $value {
            Some(v) => v,
            None => {
                let f: &dyn Fn(&ConfigItems) -> _ = &$default_fn;
                f($config)
            }
        }
    }};
    ( @build $value:expr, $config:expr, gen, $default_fn:expr ) => {{
        let f: &dyn Fn(&ConfigItems) -> _ = &$default_fn;
        f($config)
    }};

    ( @getenv $name:expr, bool ) => { get_env_bool($name) };
    ( @getenv $name:expr, $ty:ident ) => { get_env($name) };

}

//STRUCTURE:
// /// Short description (without this they won't appear on the list)
// group {
//   /// Friendly Name |> Description (Optional)
//   name: type, is_editable, action, <default_value (Optional)>
// }
//
// Where action applied when the value wasn't provided and can be:
//  def:    Use a default value
//  auto:   Value is auto generated based on other values
//  option: Value is optional
//  gen:    Value is always autogenerated and it's original value ignored
make_config! {
    folders {
        ///  Data folder |> Main data folder
        data_folder:            String, false,  def,    "data".to_string();
        /// Database URL
        database_url:           String, false,  auto,   |c| format!("{}/{}", c.data_folder, "db.sqlite3");
        /// Icon cache folder
        icon_cache_folder:      String, false,  auto,   |c| format!("{}/{}", c.data_folder, "icon_cache");
        /// Attachments folder
        attachments_folder:     String, false,  auto,   |c| format!("{}/{}", c.data_folder, "attachments");
        /// Templates folder
        templates_folder:       String, false,  auto,   |c| format!("{}/{}", c.data_folder, "templates");
        /// Session JWT key
        rsa_key_filename:       String, false,  auto,   |c| format!("{}/{}", c.data_folder, "rsa_key");
        /// Web vault folder
        web_vault_folder:       String, false,  def,    "web-vault/".to_string();
    },
    ws {
        /// Enable websocket notifications
        websocket_enabled:      bool,   false,  def,    false;
        /// Websocket address
        websocket_address:      String, false,  def,    "0.0.0.0".to_string();
        /// Websocket port
        websocket_port:         u16,    false,  def,    3012;
    },

    /// General settings
    settings {
        /// Domain URL |> This needs to be set to the URL used to access the server, including 'http[s]://'
        /// and port, if it's different than the default. Some server functions don't work correctly without this value
        domain:                 String, true,   def,    "http://localhost".to_string();
        /// Domain Set |> Indicates if the domain is set by the admin. Otherwise the default will be used.
        domain_set:             bool,   false,  def,    false;
        /// Enable web vault
        web_vault_enabled:      bool,   false,  def,    true;

        /// HIBP Api Key |> HaveIBeenPwned API Key, request it here: https://haveibeenpwned.com/API/Key
        hibp_api_key:           Pass,   true,   option;

        /// Disable icon downloads |> Set to true to disable icon downloading, this would still serve icons from
        /// $ICON_CACHE_FOLDER, but it won't produce any external network request. Needs to set $ICON_CACHE_TTL to 0,
        /// otherwise it will delete them and they won't be downloaded again.
        disable_icon_download:  bool,   true,   def,    false;
        /// Allow new signups |> Controls if new users can register. Note that while this is disabled, users could still be invited
        signups_allowed:        bool,   true,   def,    true;
        /// Require email verification on signups. This will prevent logins from succeeding until the address has been verified
        signups_verify:         bool,   true,   def,    false;
        /// If signups require email verification, automatically re-send verification email if it hasn't been sent for a while (in seconds)
        signups_verify_resend_time: u64, true,  def,    3_600;
        /// If signups require email verification, limit how many emails are automatically sent when login is attempted (0 means no limit)
        signups_verify_resend_limit: u32, true, def,    6;
        /// Allow signups only from this list of comma-separated domains
        signups_domains_whitelist: String, true, def,   "".to_string();
        /// Allow invitations |> Controls whether users can be invited by organization admins, even when signups are disabled
        invitations_allowed:    bool,   true,   def,    true;
        /// Password iterations |> Number of server-side passwords hashing iterations.
        /// The changes only apply when a user changes their password. Not recommended to lower the value
        password_iterations:    i32,    true,   def,    100_000;
        /// Show password hints |> Controls if the password hint should be shown directly in the web page.
        /// Otherwise, if email is disabled, there is no way to see the password hint
        show_password_hint:     bool,   true,   def,    true;

        /// Admin page token |> The token used to authenticate in this very same page. Changing it here won't deauthorize the current session
        admin_token:            Pass,   true,   option;

        /// Invitation organization name |> Name shown in the invitation emails that don't come from a specific organization
        invitation_org_name:    String, true,   def,    "Bitwarden_RS".to_string();
    },

    /// Advanced settings
    advanced {
        /// Client IP header |> If not present, the remote IP is used.
        /// Set to the string "none" (without quotes), to disable any headers and just use the remote IP
        ip_header:              String, true,   def,    "X-Real-IP".to_string();
        /// Internal IP header property, used to avoid recomputing each time
        _ip_header_enabled:     bool,   false,  gen,    |c| &c.ip_header.trim().to_lowercase() != "none";
        /// Positive icon cache expiry |> Number of seconds to consider that an already cached icon is fresh. After this period, the icon will be redownloaded
        icon_cache_ttl:         u64,    true,   def,    2_592_000;
        /// Negative icon cache expiry |> Number of seconds before trying to download an icon that failed again.
        icon_cache_negttl:      u64,    true,   def,    259_200;
        /// Icon download timeout |> Number of seconds when to stop attempting to download an icon.
        icon_download_timeout:  u64,    true,   def,    10;
        /// Icon blacklist Regex |> Any domains or IPs that match this regex won't be fetched by the icon service.
        /// Useful to hide other servers in the local network. Check the WIKI for more details
        icon_blacklist_regex:   String, true,   option;
        /// Icon blacklist non global IPs |> Any IP which is not defined as a global IP will be blacklisted.
        /// Usefull to secure your internal environment: See https://en.wikipedia.org/wiki/Reserved_IP_addresses for a list of IPs which it will block
        icon_blacklist_non_global_ips:  bool,   true,   def,    true;

        /// Disable Two-Factor remember |> Enabling this would force the users to use a second factor to login every time.
        /// Note that the checkbox would still be present, but ignored.
        disable_2fa_remember:   bool,   true,   def,    false;

        /// Disable authenticator time drifted codes to be valid |> Enabling this only allows the current TOTP code to be valid
        /// TOTP codes of the previous and next 30 seconds will be invalid.
        authenticator_disable_time_drift: bool, true, def, false;

        /// Require new device emails |> When a user logs in an email is required to be sent.
        /// If sending the email fails the login attempt will fail.
        require_device_email:   bool,   true,   def,     false;

        /// Reload templates (Dev) |> When this is set to true, the templates get reloaded with every request.
        /// ONLY use this during development, as it can slow down the server
        reload_templates:       bool,   true,   def,    false;
        /// Enable extended logging
        extended_logging:       bool,   false,  def,    true;
        /// Enable the log to output to Syslog
        use_syslog:             bool,   false,  def,    false;
        /// Log file path
        log_file:               String, false,  option;
        /// Log level
        log_level:              String, false,  def,    "Info".to_string();

        /// Enable DB WAL |> Turning this off might lead to worse performance, but might help if using bitwarden_rs on some exotic filesystems,
        /// that do not support WAL. Please make sure you read project wiki on the topic before changing this setting.
        enable_db_wal:          bool,   false,  def,    true;

        /// Bypass admin page security (Know the risks!) |> Disables the Admin Token for the admin page so you may use your own auth in-front
        disable_admin_token:    bool,   true,   def,    false;

        /// Allowed iframe ancestors (Know the risks!) |> Allows other domains to embed the web vault into an iframe, useful for embedding into secure intranets
        allowed_iframe_ancestors: String, true, def,    String::new();
    },

    /// Yubikey settings
    yubico: _enable_yubico {
        /// Enabled
        _enable_yubico:         bool,   true,   def,     true;
        /// Client ID
        yubico_client_id:       String, true,   option;
        /// Secret Key
        yubico_secret_key:      Pass,   true,   option;
        /// Server
        yubico_server:          String, true,   option;
    },

    /// Global Duo settings (Note that users can override them)
    duo: _enable_duo {
        /// Enabled
        _enable_duo:            bool,   true,   def,     false;
        /// Integration Key
        duo_ikey:               String, true,   option;
        /// Secret Key
        duo_skey:               Pass,   true,   option;
        /// Host
        duo_host:               String, true,   option;
        /// Application Key (generated automatically)
        _duo_akey:              Pass,   false,  option;
    },

    /// SMTP Email Settings
    smtp: _enable_smtp {
        /// Enabled
        _enable_smtp:           bool,   true,   def,     true;
        /// Host
        smtp_host:              String, true,   option;
        /// Enable SSL
        smtp_ssl:               bool,   true,   def,     true;
        /// Use explicit TLS |> Enabling this would force the use of an explicit TLS connection, instead of upgrading an insecure one with STARTTLS
        smtp_explicit_tls:      bool,   true,   def,     false;
        /// Port
        smtp_port:              u16,    true,   auto,    |c| if c.smtp_explicit_tls {465} else if c.smtp_ssl {587} else {25};
        /// From Address
        smtp_from:              String, true,   def,     String::new();
        /// From Name
        smtp_from_name:         String, true,   def,     "Bitwarden_RS".to_string();
        /// Username
        smtp_username:          String, true,   option;
        /// Password
        smtp_password:          Pass,   true,   option;
        /// Json form auth mechanism |> Defaults for ssl is "Plain" and "Login" and nothing for non-ssl connections. Possible values: ["Plain", "Login", "Xoauth2"]
        smtp_auth_mechanism:    String, true,   option;
        /// SMTP connection timeout |> Number of seconds when to stop trying to connect to the SMTP server
        smtp_timeout:           u64,     true,   def,     15;
    },

    /// Email 2FA Settings
    email_2fa: _enable_email_2fa {
        /// Enabled |> Disabling will prevent users from setting up new email 2FA and using existing email 2FA configured
        _enable_email_2fa:      bool,   true,   auto,    |c| c._enable_smtp && c.smtp_host.is_some();
        /// Token number length |> Length of the numbers in an email token. Minimum of 6. Maximum is 19.
        email_token_size:       u32,    true,   def,      6;
        /// Token expiration time |> Maximum time in seconds a token is valid. The time the user has to open email client and copy token.
        email_expiration_time:  u64,    true,   def,      600;
        /// Maximum attempts |> Maximum attempts before an email token is reset and a new email will need to be sent
        email_attempts_limit:   u64,    true,   def,      3;
    },
}

fn validate_config(cfg: &ConfigItems) -> Result<(), Error> {
    let db_url = cfg.database_url.to_lowercase();
    if cfg!(feature = "sqlite") && (db_url.starts_with("mysql:") || db_url.starts_with("postgresql:")) {
        err!("`DATABASE_URL` is meant for MySQL or Postgres, while this server is meant for SQLite")
    }

    if cfg!(feature = "mysql") && !db_url.starts_with("mysql:") {
        err!("`DATABASE_URL` should start with mysql: when using the MySQL server")
    }

    if cfg!(feature = "postgresql") && !db_url.starts_with("postgresql:") {
        err!("`DATABASE_URL` should start with postgresql: when using the PostgreSQL server")
    }

    if let Some(ref token) = cfg.admin_token {
        if token.trim().is_empty() && !cfg.disable_admin_token {
            err!("`ADMIN_TOKEN` is enabled but has an empty value. To enable the admin page without token, use `DISABLE_ADMIN_TOKEN`")
        }
    }

    if cfg._enable_duo
        && (cfg.duo_host.is_some() || cfg.duo_ikey.is_some() || cfg.duo_skey.is_some())
        && !(cfg.duo_host.is_some() && cfg.duo_ikey.is_some() && cfg.duo_skey.is_some())
    {
        err!("All Duo options need to be set for global Duo support")
    }

    if cfg._enable_yubico && cfg.yubico_client_id.is_some() != cfg.yubico_secret_key.is_some() {
        err!("Both `YUBICO_CLIENT_ID` and `YUBICO_SECRET_KEY` need to be set for Yubikey OTP support")
    }

    if cfg._enable_smtp {
        if cfg.smtp_host.is_some() == cfg.smtp_from.is_empty() {
            err!("Both `SMTP_HOST` and `SMTP_FROM` need to be set for email support")
        }

        if cfg.smtp_username.is_some() != cfg.smtp_password.is_some() {
            err!("Both `SMTP_USERNAME` and `SMTP_PASSWORD` need to be set to enable email authentication")
        }

        if cfg._enable_email_2fa && (!cfg._enable_smtp || cfg.smtp_host.is_none()) {
            err!("To enable email 2FA, SMTP must be configured")
        }

        if cfg._enable_email_2fa && cfg.email_token_size < 6 {
            err!("`EMAIL_TOKEN_SIZE` has a minimum size of 6")
        }

        if cfg._enable_email_2fa && cfg.email_token_size > 19 {
            err!("`EMAIL_TOKEN_SIZE` has a maximum size of 19")
        }
    }

    Ok(())
}

impl Config {
    pub fn load() -> Result<Self, Error> {
        // Loading from env and file
        let _env = ConfigBuilder::from_env();
        let _usr = ConfigBuilder::from_file(&CONFIG_FILE).unwrap_or_default();

        // Create merged config, config file overwrites env
        let builder = _env.merge(&_usr, true);

        // Fill any missing with defaults
        let config = builder.build();
        validate_config(&config)?;

        Ok(Config {
            inner: RwLock::new(Inner {
                rocket_shutdown_handle: None,
                ws_shutdown_handle: None,
                templates: load_templates(&config.templates_folder),
                config,
                _env,
                _usr,
            }),
        })
    }

    pub fn update_config(&self, other: ConfigBuilder) -> Result<(), Error> {
        // Remove default values
        //let builder = other.remove(&self.inner.read().unwrap()._env);

        // TODO: Remove values that are defaults, above only checks those set by env and not the defaults
        let builder = other;

        // Serialize now before we consume the builder
        let config_str = serde_json::to_string_pretty(&builder)?;

        // Prepare the combined config
        let config = {
            let env = &self.inner.read().unwrap()._env;
            env.merge(&builder, false).build()
        };
        validate_config(&config)?;

        // Save both the user and the combined config
        {
            let mut writer = self.inner.write().unwrap();
            writer.config = config;
            writer._usr = builder;
        }

        //Save to file
        use std::{fs::File, io::Write};
        let mut file = File::create(&*CONFIG_FILE)?;
        file.write_all(config_str.as_bytes())?;

        Ok(())
    }

    pub fn update_config_partial(&self, other: ConfigBuilder) -> Result<(), Error> {
        let builder = {
            let usr = &self.inner.read().unwrap()._usr;
            usr.merge(&other, false)
        };
        self.update_config(builder)
    }

    pub fn can_signup_user(&self, email: &str) -> bool {
        let e: Vec<&str> = email.rsplitn(2, '@').collect();
        if e.len() != 2 || e[0].is_empty() || e[1].is_empty() {
            warn!("Failed to parse email address '{}'", email);
            return false;
        }
        
        // Allow signups if the whitelist is empty/not configured
        // (it doesn't contain any domains), or if it matches at least
        // one domain.
        let whitelist_str = self.signups_domains_whitelist();
        ( whitelist_str.is_empty() && CONFIG.signups_allowed() )|| whitelist_str.split(',').filter(|s| !s.is_empty()).any(|d| d == e[0])
    }

    pub fn delete_user_config(&self) -> Result<(), Error> {
        crate::util::delete_file(&CONFIG_FILE)?;

        // Empty user config
        let usr = ConfigBuilder::default();

        // Config now is env + defaults
        let config = {
            let env = &self.inner.read().unwrap()._env;
            env.build()
        };

        // Save configs
        {
            let mut writer = self.inner.write().unwrap();
            writer.config = config;
            writer._usr = usr;
        }

        Ok(())
    }

    pub fn private_rsa_key(&self) -> String {
        format!("{}.der", CONFIG.rsa_key_filename())
    }
    pub fn private_rsa_key_pem(&self) -> String {
        format!("{}.pem", CONFIG.rsa_key_filename())
    }
    pub fn public_rsa_key(&self) -> String {
        format!("{}.pub.der", CONFIG.rsa_key_filename())
    }
    pub fn mail_enabled(&self) -> bool {
        let inner = &self.inner.read().unwrap().config;
        inner._enable_smtp && inner.smtp_host.is_some()
    }

    pub fn get_duo_akey(&self) -> String {
        if let Some(akey) = self._duo_akey() {
            akey
        } else {
            let akey = crate::crypto::get_random_64();
            let akey_s = data_encoding::BASE64.encode(&akey);

            // Save the new value
            let mut builder = ConfigBuilder::default();
            builder._duo_akey = Some(akey_s.clone());
            self.update_config_partial(builder).ok();

            akey_s
        }
    }

    pub fn render_template<T: serde::ser::Serialize>(
        &self,
        name: &str,
        data: &T,
    ) -> Result<String, crate::error::Error> {
        if CONFIG.reload_templates() {
            warn!("RELOADING TEMPLATES");
            let hb = load_templates(CONFIG.templates_folder());
            hb.render(name, data).map_err(Into::into)
        } else {
            let hb = &CONFIG.inner.read().unwrap().templates;
            hb.render(name, data).map_err(Into::into)
        }
    }

    pub fn set_rocket_shutdown_handle(&self, handle: rocket::shutdown::ShutdownHandle) {
        self.inner.write().unwrap().rocket_shutdown_handle = Some(handle);
    }

    pub fn set_ws_shutdown_handle(&self, handle: ws::Sender) {
        self.inner.write().unwrap().ws_shutdown_handle = Some(handle);
    }

    pub fn shutdown(&self) {
        match self.inner.read() {
            Ok(c) => {
                warn!("Initiating shutdown!");
                c.ws_shutdown_handle.clone().map(|s| s.shutdown());
                // Wait a bit before stopping the web server
                std::thread::sleep(std::time::Duration::from_secs(1));
                c.rocket_shutdown_handle.clone().map(|s| s.shutdown());
            }
            Err(_) => {}
        }
    }
}

use handlebars::{Context, Handlebars, Helper, HelperResult, Output, RenderContext, RenderError, Renderable};

fn load_templates<P>(path: P) -> Handlebars<'static>
where
    P: AsRef<std::path::Path>,
{
    let mut hb = Handlebars::new();
    // Error on missing params
    hb.set_strict_mode(true);
    // Register helpers
    hb.register_helper("case", Box::new(case_helper));
    hb.register_helper("jsesc", Box::new(js_escape_helper));

    macro_rules! reg {
        ($name:expr) => {{
            let template = include_str!(concat!("static/templates/", $name, ".hbs"));
            hb.register_template_string($name, template).unwrap();
        }};
        ($name:expr, $ext:expr) => {{
            reg!($name);
            reg!(concat!($name, $ext));
        }};
    }

    // First register default templates here
    reg!("email/change_email", ".html");
    reg!("email/delete_account", ".html");
    reg!("email/invite_accepted", ".html");
    reg!("email/invite_confirmed", ".html");
    reg!("email/new_device_logged_in", ".html");
    reg!("email/pw_hint_none", ".html");
    reg!("email/pw_hint_some", ".html");
    reg!("email/send_org_invite", ".html");
    reg!("email/twofactor_email", ".html");
    reg!("email/verify_email", ".html");
    reg!("email/welcome", ".html");
    reg!("email/welcome_must_verify", ".html");

    reg!("admin/base");
    reg!("admin/login");
    reg!("admin/page");

    // And then load user templates to overwrite the defaults
    // Use .hbs extension for the files
    // Templates get registered with their relative name
    hb.register_templates_directory(".hbs", path).unwrap();

    hb
}

fn case_helper<'reg, 'rc>(
    h: &Helper<'reg, 'rc>,
    r: &'reg Handlebars,
    ctx: &'rc Context,
    rc: &mut RenderContext<'reg, 'rc>,
    out: &mut dyn Output,
) -> HelperResult {
    let param = h
        .param(0)
        .ok_or_else(|| RenderError::new("Param not found for helper \"case\""))?;
    let value = param.value().clone();

    if h.params().iter().skip(1).any(|x| x.value() == &value) {
        h.template().map(|t| t.render(r, ctx, rc, out)).unwrap_or(Ok(()))
    } else {
        Ok(())
    }
}

fn js_escape_helper<'reg, 'rc>(
    h: &Helper<'reg, 'rc>,
    _r: &'reg Handlebars,
    _ctx: &'rc Context,
    _rc: &mut RenderContext<'reg, 'rc>,
    out: &mut dyn Output,
) -> HelperResult {
    let param = h
        .param(0)
        .ok_or_else(|| RenderError::new("Param not found for helper \"js_escape\""))?;

    let value = param
        .value()
        .as_str()
        .ok_or_else(|| RenderError::new("Param for helper \"js_escape\" is not a String"))?;

    let escaped_value = value.replace('\\', "").replace('\'', "\\x22").replace('\"', "\\x27");
    let quoted_value = format!("&quot;{}&quot;", escaped_value);

    out.write(&quoted_value)?;
    Ok(())
}
