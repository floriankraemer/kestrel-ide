#include "vcs_status_color_proxy.h"

#include "theme.h"
#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QVariant>

namespace ui_shell {

VcsStatusColorProxy::VcsStatusColorProxy(VcsService *vcsService, int pathRole, QObject *parent)
  : QIdentityProxyModel(parent)
  , vcsService_(vcsService)
  , pathRole_(pathRole)
{
}

QVariant VcsStatusColorProxy::data(const QModelIndex &index, int role) const
{
    if (role != Qt::ForegroundRole || vcsService_ == nullptr) {
        return QIdentityProxyModel::data(index, role);
    }

    const QString path = QIdentityProxyModel::data(index, pathRole_).toString();
    if (path.isEmpty()) {
        return QIdentityProxyModel::data(index, role);
    }

    const FfiChangedFile status = vcsService_->fileStatus(path);
    if (status.path.isEmpty()) {
        // No pending change for this path (directories included: `git
        // status` never reports one for a directory itself) — fall back to
        // whatever the identity model would have answered.
        return QIdentityProxyModel::data(index, role);
    }
    // The unstaged kind wins when both are set: it is what still needs
    // attention (staged is already "done"), the same precedence
    // `changes_panel.cpp` gives it by listing Staged above Unstaged but
    // colouring both from the same table — here there is only one row to
    // colour, so unstaged, the more actionable state, takes it.
    const FfiChangeKind kind =
      status.unstaged != FfiChangeKind::None ? status.unstaged : status.staged;
    const QColor color = changeKindColor(kind);
    return color.isValid() ? QVariant::fromValue(color) : QIdentityProxyModel::data(index, role);
}

} // namespace ui_shell
