#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <functional>

class QWidget;

namespace ui_shell {

// Settings > Containers > Registries (C7, ADR-0055): named registries
// (Docker Hub, GitLab, Docker V2, Generic, plus GHCR/Quay presets), their
// address/username/GitLab-project fields, and a password/token field that
// is stored via the OS keychain on commit — never written to `settings.toml`
// (`AppSettings::registries()`/`saveRegistries()` carry no secret field at
// all, `container_registry::secrets::SecretStore` is where the password
// goes).
//
// Same commit-on-OK shape as `ContainersPage`/`buildContainersPage`: a
// registry is edited as a draft list and only written back (rows *and*
// touched secrets) when the dialog is accepted.
struct RegistriesPage
{
    QWidget *widget;
    std::function<void()> commit;
};

// Humble view: which registry kind supports which field, what "Test
// connection" says, and how a keychain-unavailable error reads are all
// decided behind `container_registry`/`container_core::registry_ref`
// (through `AppSettings`). This file only renders the list, the per-kind
// form, and the password field's keychain hint.
RegistriesPage buildRegistriesPage(QWidget *parent, AppSettings *appSettings);

} // namespace ui_shell
