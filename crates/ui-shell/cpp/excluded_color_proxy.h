#pragma once

#include <QIdentityProxyModel>

namespace ui_shell {

// Answers Qt::ForegroundRole with the theme's muted colour for a row whose
// `IsExcluded` role (ProjectTreeModel::Roles::IsExcluded) is true — a
// folder marked "Mark Directory as Excluded", or anything shown beneath it
// (ADR-0064: the tree is lazy and still lists an excluded folder's
// contents, JetBrains-style). Every other row falls through to the source
// model's own answer.
//
// Mirrors VcsStatusColorProxy's shape exactly (vcs_status_color_proxy.h):
// one role in, one role translated, everything else passed through
// unchanged.
class ExcludedColorProxy : public QIdentityProxyModel
{
    Q_OBJECT

public:
    ExcludedColorProxy(int isExcludedRole, QObject *parent);

    QVariant data(const QModelIndex &index, int role) const override;

private:
    int isExcludedRole_;
};

} // namespace ui_shell
