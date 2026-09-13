#pragma once

#include <QIdentityProxyModel>

class VcsService;

namespace ui_shell {

// Answers Qt::ForegroundRole from the source model's own path role, by
// asking `VcsService::fileStatus` — the same colour table
// (`changeKindColor`, theme.h) the Changes dock's status letter and the
// editor tab title (R6) use, so the three never disagree about what
// "modified" looks like.
//
// Mirrors IconDecorationProxy's shape exactly (icon_decoration_proxy.h):
// one role in, one role translated, everything else passed through
// unchanged — a Rust-backed model stays free of Qt colour/pixmap types,
// same reason that proxy exists.
class VcsStatusColorProxy : public QIdentityProxyModel
{
    Q_OBJECT

public:
    // `vcsService` may be null (no VCS at all for this project) — every
    // row then answers the identity model's own colour, same as an empty
    // path would.
    VcsStatusColorProxy(VcsService *vcsService, int pathRole, QObject *parent);

    QVariant data(const QModelIndex &index, int role) const override;

private:
    VcsService *vcsService_;
    int pathRole_;
};

} // namespace ui_shell
