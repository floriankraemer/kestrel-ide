//! Go to Test / Test Subject (PHPUnit editor runs plan, E7): translation
//! only — which files a class and its tests map to is
//! `php_core::test_navigation`'s rule; this moves the request off the Qt
//! thread and the answer back.

use std::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::{QString, QStringList};

use super::gutter::is_php_file;
use crate::bridge::ffi;

impl ffi::TestService {
    pub fn is_test_file(&self, path: &QString, text: &QString) -> bool {
        let path = path.to_string();
        is_php_file(&path)
            && php_core::test_navigation::is_test_file(
                std::path::Path::new(&path),
                &text.to_string(),
            )
    }

    pub fn request_test_navigation(mut self: Pin<&mut Self>, path: &QString, text: &QString) {
        let path = std::path::PathBuf::from(path.to_string());
        let root = super::current_project_root();
        let (Some(root), true) = (root, is_php_file(&path.to_string_lossy())) else {
            let none = QStringList::default;
            self.as_mut()
                .test_navigation_ready(false, QString::default(), none(), none());
            return;
        };
        let text = text.to_string();
        let qt_thread = self.qt_thread();
        std::thread::spawn(move || {
            let navigation = php_core::test_navigation::navigate(&root, &path, &text);
            let _ = qt_thread.queue(move |service: Pin<&mut ffi::TestService>| {
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
                let name = QString::from(navigation.name.as_str());
                service.test_navigation_ready(navigation.from_test, name, paths, labels);
            });
        });
    }
}
