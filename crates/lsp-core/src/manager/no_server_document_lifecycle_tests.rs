use super::*;

fn manager() -> LspManager {
    let (manager, _rx) = LspManager::new(crate::diagnostics::uri_from_path("/tmp/proj"));
    manager
}

#[test]
fn did_open_with_no_server_records_the_document_and_returns_no_server() {
    let manager = manager();
    let uri = "file:///tmp/proj/pom.xml";
    let err = manager.did_open(uri, "xml", "<project/>").unwrap_err();
    assert!(matches!(err, LspError::NoServer(lang) if lang == "xml"));
    // The document is still tracked, even though no server was
    // notified — did_change below depends on this.
    assert!(manager.documents.lock().unwrap().contains_key(uri));
}

#[test]
fn did_change_with_no_server_bumps_the_version_and_returns_no_server() {
    let manager = manager();
    let uri = "file:///tmp/proj/pom.xml";
    manager.did_open(uri, "xml", "<project/>").unwrap_err();
    let err = manager
        .did_change(uri, "<project><x/></project>")
        .unwrap_err();
    assert!(matches!(err, LspError::NoServer(lang) if lang == "xml"));
}

#[test]
fn did_close_with_no_server_forgets_the_document_and_returns_no_server() {
    let manager = manager();
    let uri = "file:///tmp/proj/pom.xml";
    manager.did_open(uri, "xml", "<project/>").unwrap_err();
    let err = manager.did_close(uri).unwrap_err();
    assert!(matches!(err, LspError::NoServer(lang) if lang == "xml"));
    assert!(!manager.documents.lock().unwrap().contains_key(uri));
}
