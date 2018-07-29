// TODO:
// Caching of assets
// CORS
// evaluate.json
// Logging

use http::{header, Method, Response};
use std::{
    convert::TryInto,
    io,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    fs::File,
    prelude::{future::Either, stream, Future},
};
use tower_web::{
    codegen::bytes::BytesMut,
    extract::http_date_time::HttpDateTime,
    middleware::cors::{AllowedOrigins, CorsBuilder},
    util::buf_stream::StdStream,
    ServiceBuilder,
};

use ::{
    CachedSandbox,
    ClippyRequest,
    ClippyResponse,
    CompileRequest,
    CompileResponse,
    Config,
    Error,
    EvaluateRequest,
    EvaluateResponse,
    ExecuteRequest,
    ExecuteResponse,
    FormatRequest,
    FormatResponse,
    MetaCratesResponse,
    MetaGistCreateRequest,
    MetaGistResponse,
    MetaVersionResponse,
    MiriRequest,
    MiriResponse,
    ONE_DAY_IN_SECONDS,
    ONE_HOUR_IN_SECONDS,
    ONE_YEAR_IN_SECONDS,
    Result,
    Sandbox,
    SandboxCache,
    gist,
};

#[derive(Debug)]
struct Index(FileX);

impl Index {
    fn new(base: PathBuf) -> Self {
        Index(FileX::new(base))
    }
}

#[derive(Debug)]
struct Assets(FileX);

impl Assets {
    fn new(mut base: PathBuf) -> Self {
        base.push("assets");
        Assets(FileX::new(base))
    }
}

#[derive(Debug)]
struct SandboxFixme;

#[derive(Debug, Default)]
struct Meta {
    cache: SandboxCache,
}

impl Meta {
    fn cached(&self, sandbox: Sandbox) -> CachedSandbox {
        CachedSandbox {
            sandbox,
            cache: &self.cache,
        }
    }
}

#[derive(Debug, Default)]
struct Gist {
    token: String,
}

impl Gist {
    fn new(token: String) -> Self {
        Self { token }
    }
}

type Empty = StdStream<stream::Empty<io::Cursor<BytesMut>, io::Error>>;
type MaybeFile = Either<File, Empty>;
type FileResponse = Response<MaybeFile>;

fn empty() -> Empty {
    StdStream::new(stream::empty())
}

#[derive(Debug)]
struct FileX {
    base: PathBuf,
}

impl FileX {
    fn new(base: PathBuf) -> Self {
        Self { base }
    }

    fn file<P>(
        &self,
        relative_path: P,
        if_modified_since: Option<HttpDateTime>,
    ) -> impl Future<Item = FileResponse, Error = io::Error> + Send
    where
        P: AsRef<Path>,
    {
        let relative_path = relative_path.as_ref();

        debug!("File is {}", relative_path.display());

        let requested_path = self.base.join(relative_path);

        let gz_path = {
            let mut current_ext = requested_path
                .extension()
                .unwrap_or_default()
                .to_os_string();
            current_ext.push(".gz");
            requested_path.with_extension(current_ext)
        };

        debug!(
            "Looking for {} instead of {}",
            gz_path.display(),
            requested_path.display()
        );

        // TODO: Guess the content type
        let ct = match relative_path.extension() {
            Some(c) if c == "html" => "text/html",
            Some(c) if c == "css" => "text/css",
            _ => "application/octet-stream",
        };

        File::open(gz_path)
            .map(|f| (f, true))
            .or_else(|_| File::open(requested_path).map(|f| (f, false)))
            .and_then(|(f, gzipped)| f.metadata().map(move |(f, md)| (f, md, gzipped)))
            .map(move |(f, md, gzipped)| {
                let last_modified = md.modified().map(HttpDateTime::from);

                let mut resp = Response::builder();

                if let (Some(client), Ok(server)) = (&if_modified_since, &last_modified) {
                    debug!("Client has an if-modified-since date of {:?}", client);
                    debug!("Server has a last-modified date of      {:?}", server);

                    if client >= server {
                        debug!("File unchanged, returning 304");
                        return resp
                            .status(304)
                            .body(Either::B(empty()))
                            .expect("Did not create response");
                    }
                }

                resp.status(200).header("Content-Type", ct);

                if gzipped {
                    debug!("Found the gzipped version of the asset");
                    resp.header("Content-Encoding", "gzip");
                }

                if let Ok(last_modified) = last_modified {
                    debug!("File had a modification time");
                    resp.header("Last-Modified", last_modified);
                }

                resp.body(Either::A(f)).expect("Did not create response")
            }).or_else(|e| {
                debug!("AN ERROR {}", e);

                // Only for certain errors?

                Ok(Response::builder()
                    .status(404)
                    .body(Either::B(empty()))
                    .expect("Did not create response"))
            }).map_err(|e| {
                debug!("AN ERROR {}", e);
                e
            })
    }
}

impl_web! {
    impl Index {
        #[get("/")]
        fn index(
            &self,
            if_modified_since: Option<HttpDateTime>,
        ) -> impl Future<Item = FileResponse, Error = io::Error> + Send {
            self.0.file("index.html", if_modified_since)
        }

        #[get("/help")]
        fn help(
            &self,
            if_modified_since: Option<HttpDateTime>,
        ) -> impl Future<Item = FileResponse, Error = io::Error> + Send {
            self.index(if_modified_since)
        }
    }

    impl Assets {
        #[get("/assets/*asset")]
        fn asset(
            &self,
            asset: PathBuf,
            if_modified_since: Option<HttpDateTime>,
        ) -> impl Future<Item = FileResponse, Error = io::Error> + Send {
            self.0.file(asset, if_modified_since)
        }
    }

    impl SandboxFixme {
        #[post("/execute")]
        #[content_type("application/json")]
        fn execute(&self, body: ExecuteRequest) -> Result<ExecuteResponse> {
            Sandbox::new()?
                .execute(&body.try_into()?)
                .map(ExecuteResponse::from)
                .map_err(Error::Sandbox)
        }

        #[post("/compile")]
        #[content_type("application/json")]
        fn compile(&self, body: CompileRequest) -> Result<CompileResponse> {
            Sandbox::new()?
                .compile(&body.try_into()?)
                .map(CompileResponse::from)
                .map_err(Error::Sandbox)
        }

        #[post("/format")]
        #[content_type("application/json")]
        fn format(&self, body: FormatRequest) -> Result<FormatResponse> {
            Sandbox::new()?
                .format(&body.try_into()?)
                .map(FormatResponse::from)
                .map_err(Error::Sandbox)
        }

        #[post("/clippy")]
        #[content_type("application/json")]
        fn clippy(&self, body: ClippyRequest) -> Result<ClippyResponse> {
            Sandbox::new()?
                .clippy(&body.into())
                .map(ClippyResponse::from)
                .map_err(Error::Sandbox)
        }

        #[post("/miri")]
        #[content_type("application/json")]
        fn miri(&self, body: MiriRequest) -> Result<MiriResponse> {
            Sandbox::new()?
                .miri(&body.into())
                .map(MiriResponse::from)
                .map_err(Error::Sandbox)
        }

        // This is a backwards compatibilty shim. The Rust homepage and the
        // documentation use this to run code in place.
        #[post("/evaluate.json")]
        #[content_type("application/json")]
        fn evaluate(&self, body: EvaluateRequest) -> Result<EvaluateResponse> {
            Sandbox::new()?
                .execute(&body.try_into()?)
                .map(EvaluateResponse::from)
                .map_err(Error::Sandbox)
        }
    }

    impl Meta {
        #[get("/meta/crates")]
        #[content_type("application/json")]
        fn meta_crates(&self) -> Result<MetaCratesResponse> {
            self.cached(Sandbox::new()?)
                .crates()
                .map(MetaCratesResponse::from)
        }

        #[get("/meta/version/stable")]
        #[content_type("application/json")]
        fn meta_version_stable(&self) -> Result<MetaVersionResponse> {
            self.cached(Sandbox::new()?)
                .version_stable()
                .map(MetaVersionResponse::from)
        }

        #[get("/meta/version/beta")]
        #[content_type("application/json")]
        fn meta_version_beta(&self) -> Result<MetaVersionResponse> {
            self.cached(Sandbox::new()?)
                .version_beta()
                .map(MetaVersionResponse::from)
        }

        #[get("/meta/version/nightly")]
        #[content_type("application/json")]
        fn meta_version_nightly(&self) -> Result<MetaVersionResponse> {
            self.cached(Sandbox::new()?)
                .version_nightly()
                .map(MetaVersionResponse::from)
        }
    }

    impl Gist {
        #[post("/meta/gist")]
        #[content_type("application/json")]
        fn create(
            &self,
            body: MetaGistCreateRequest,
        ) -> impl Future<Item = MetaGistResponse, Error = Error> + Send {
            gist::create_future(self.token.clone(), body.code)
                .map(|gist| MetaGistResponse::from(gist))
                .map_err(|e| unimplemented!("FIXME {:?}", e))
        }

        #[get("/meta/gist/:id")]
        #[content_type("application/json")]
        fn show(&self, id: String) -> impl Future<Item = MetaGistResponse, Error = Error> + Send {
            gist::load_future(self.token.clone(), &id)
                .map(|gist| MetaGistResponse::from(gist))
                .map_err(|e| unimplemented!("FIXME {:?}", e))
        }
    }
}

pub fn run(config: Config) {
    let addr = SocketAddr::new(config.address.parse().unwrap(), config.port).into();
    info!("[Tower-Web] Starting the server on http://{}", addr);

    let builder = ServiceBuilder::new()
        .resource(Index::new(config.root.clone()))
        .resource(Assets::new(config.root))
        .resource(SandboxFixme)
        .resource(Meta::default())
        .resource(Gist::new(config.gh_token));

    // if config.cors_enabled {
        let cors = CorsBuilder::new()
            .allow_origins(AllowedOrigins::Any { allow_null: true })
            .allow_headers(vec![header::CONTENT_TYPE])
            .allow_methods(vec![Method::GET, Method::POST])
            .allow_credentials(false)
            .max_age(Duration::from_secs(ONE_HOUR_IN_SECONDS as u64))
            .prefer_wildcard(true)
            .build();

        let builder = builder.middleware(cors);
    // }

    builder.run(&addr).unwrap();
}
