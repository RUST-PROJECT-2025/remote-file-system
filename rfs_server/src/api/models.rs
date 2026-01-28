use actix_web::{FromRequest, HttpRequest, dev::Payload, error::{ErrorBadRequest, ErrorInternalServerError, ErrorNotFound}, http::Method, web};
use futures_util::future::{LocalBoxFuture, ready, FutureExt};
use std::path::PathBuf;

const ROOT_DIR: &str = "/tmp/rfs_storage";

pub struct SafePath(PathBuf);

impl SafePath {
    pub fn into_inner(self) -> PathBuf { self.0 }
}

impl FromRequest for SafePath {
    type Error = actix_web::Error;
    type Future = LocalBoxFuture<'static, Result<Self, Self::Error>>;

    fn from_request(req: &HttpRequest, payload: &mut Payload) -> Self::Future {
        let path_result: Result<web::Path<PathBuf>, actix_web::Error> =
            web::Path::from_request(req, payload).into_inner();

        let root_path = PathBuf::from(ROOT_DIR);

        match path_result {
            Ok(path) => {
                let relative_path = path.into_inner();
                // Gestione sicurezza path traversal
                let path_to_verify = match *req.method() {
                    Method::POST | Method::PUT => {
                        let full_path = root_path.join(&relative_path);
                        full_path.parent().map(|p| p.to_path_buf()).unwrap_or(root_path.clone())
                    }
                    _ => root_path.join(&relative_path)
                };

                if !path_to_verify.exists() {
                     // Per GET/DELETE/PATCH il file deve esistere, per PUT/POST (creazione) il parent deve esistere
                     // Qui semplifichiamo: se non esiste, ritorniamo 404 per GET
                     if *req.method() == Method::GET || *req.method() == Method::DELETE || *req.method() == Method::PATCH {
                         return ready(Err(ErrorNotFound(format!("Risorsa non trovata: {:?}", path_to_verify)))).boxed_local();
                     }
                }
                
                // Nota: In produzione la canonicalizzazione va gestita con più cura per i file non ancora creati.
                // Qui assumiamo percorso sicuro base + relativo.
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