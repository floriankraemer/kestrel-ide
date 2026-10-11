//! Go to Test / Test Subject (PHPUnit editor runs plan, E7): translation
//! only — which files a class and its tests map to is
//! `php_core::test_navigation`'s rule, and which answer still counts is its
//! `NavigationRequests`', and the covering tests come from its
//! `CoversCache`; this moves the search off the Qt thread and the answer
//! back.

use std::path::{Path, PathBuf};
use std::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::{QString, QStringList};

use super::gutter::is_php_file;
use crate::bridge::ffi;

/// One Go to Test search: the project root, the file and its text.
pub(super) struct NavigationRequest {
    root: PathBuf,
    file: PathBuf,
    text: String,
}

pub(super) type NavigationRequests =
    php_core::test_navigation::NavigationRequests<NavigationRequest>;

impl ffi::TestService {
    pub fn is_test_file(&self, path: &QString, text: &QString) -> bool {
        let path = path.to_string();
        // The gutter's parse of this text, cached, rather than a parse per
        // menu opening.
        is_php_file(&path)
            && (php_core::test_navigation::is_named_test(Path::new(&path))
                || !self.markers_of(&path, &text.to_string()).is_empty())
    }

    pub fn request_test_navigation(mut self: Pin<&mut Self>, path: &QString, text: &QString) {
        let file = PathBuf::from(path.to_string());
        let status = match super::current_project_root() {
            None => Err(ffi::FfiTestNavigationStatus::NoProject),
            Some(_) if !is_php_file(&file.to_string_lossy()) => {
                Err(ffi::FfiTestNavigationStatus::NotPhp)
            }
            Some(root) => Ok(root),
        };
        let root = match status {
            Ok(root) => root,
            Err(status) => {
                self.navigation.borrow_mut().cancel();
                self.as_mut().test_navigation_ready(ffi::FfiTestNavigation {
                    status,
                    from_test: false,
                    still_indexing: false,
                    name: QString::default(),
                    paths: QStringList::default(),
                    labels: QStringList::default(),
                });
                return;
            }
        };
        let request = NavigationRequest {
            root,
            file,
            text: text.to_string(),
        };
        let started = self.navigation.borrow_mut().request(request);
        if let Some((token, request)) = started {
            self.as_mut().search_navigation(token, request);
        }
    }

    pub fn cancel_test_navigation(&self) {
        self.navigation.borrow_mut().cancel();
    }

    /// Search for `request` on a worker thread, then answer if it is still
    /// wanted and start the request that waited for it.
    fn search_navigation(self: Pin<&mut Self>, token: u64, request: NavigationRequest) {
        let qt_thread = self.qt_thread();
        let covers = std::sync::Arc::clone(&self.covers);
        std::thread::spawn(move || {
            let navigation = php_core::test_navigation::navigate(
                &request.root,
                &request.file,
                &request.text,
                &covers,
            );
            let root = request.root;
            let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::TestService>| {
                let (wanted, next) = service.navigation.borrow_mut().finish(token);
                if let Some((token, request)) = next {
                    service.as_mut().search_navigation(token, request);
                }
                // A project closed meanwhile has no files to open.
                if !wanted || super::current_project_root().as_deref() != Some(root.as_path()) {
                    return;
                }
                let paths: QStringList = navigation
                    .files
                    .iter()
                    .map(|file| QString::from(root.join(file).to_string_lossy().as_ref()))
                    .collect();
                let labels: QStringList = navigation
                    .files
                    .iter()
                    .map(|file| QString::from(file.as_str()))
                    .collect();
                let status = if navigation.files.is_empty() {
                    ffi::FfiTestNavigationStatus::NoneFound
                } else {
                    ffi::FfiTestNavigationStatus::Found
                };
                service.test_navigation_ready(ffi::FfiTestNavigation {
                    status,
                    from_test: navigation.from_test,
                    still_indexing: navigation.still_indexing,
                    name: QString::from(navigation.name.as_str()),
                    paths,
                    labels,
                });
            });
        });
    }
}
