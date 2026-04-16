use actix_web::{FromRequest, HttpRequest, dev::Payload, error::{ErrorBadRequest, ErrorInternalServerError, ErrorNotFound}, http::Method, web};
use futures_util::future::{LocalBoxFuture, ready, FutureExt};
use std::path::PathBuf;

const ROOT_DIR: &str = "/tmp/rfs_storage";

// Wrapper per garantire che i path siano sempre sotto ROOT_DIR e prevenire path traversal
// viene eseguito automaticamente da actix quando si usa SafePath come extractor nei handler
pub struct SafePath(PathBuf);

impl SafePath {
    pub fn into_inner(self) -> PathBuf { self.0 }
}

impl FromRequest for SafePath {
    type Error = actix_web::Error;
    type Future = LocalBoxFuture<'static, Result<Self, Self::Error>>;

    fn from_request(req: &HttpRequest, payload: &mut Payload) -> Self::Future {
        // prendiamo la parte del path {tail:.*} come PathBuf
        let path_result: Result<web::Path<PathBuf>, actix_web::Error> =
            web::Path::from_request(req, payload).into_inner();

        let root_path = PathBuf::from(ROOT_DIR);

        match path_result {
            Ok(path) => {
                let relative_path = path.into_inner();
                // Gestione sicurezza path traversal
                let path_to_verify = match *req.method() {
                    Method::POST | Method::PUT => {
                        //  per PUT/POST (creazione) il parent deve esistere
                        let full_path = root_path.join(&relative_path);
                        full_path.parent().map(|p| p.to_path_buf()).unwrap_or(root_path.clone())
                    }
                    _ => root_path.join(&relative_path)
                };

                if !path_to_verify.exists() {
                     // Per GET/DELETE/PATCH il file deve esistere
                     // se non esiste, ritorniamo 404 per GET
                     if *req.method() == Method::GET || *req.method() == Method::DELETE || *req.method() == Method::PATCH {
                         return ready(Err(ErrorNotFound(format!("Risorsa non trovata: {:?}", path_to_verify)))).boxed_local();
                     }
                }
                
                // Verifichiamo che il path finale sia sempre sotto ROOT_DIR
                let final_path = root_path.join(&relative_path);
                if !final_path.starts_with(&root_path) {
                    return ready(Err(ErrorBadRequest("Path Traversal detected"))).boxed_local();
                }

                ready(Ok(SafePath(final_path))).boxed_local()
            }
            Err(e) => ready(Err(e)).boxed_local(),
        }
    }
}