#include "excluded_color_proxy.h"

#include "theme.h"

#include <QVariant>

namespace ui_shell {

ExcludedColorProxy::ExcludedColorProxy(int isExcludedRole, QObject *parent)
  : QIdentityProxyModel(parent)
  , isExcludedRole_(isExcludedRole)
{
}

QVariant ExcludedColorProxy::data(const QModelIndex &index, int role) const
{
    if (role != Qt::ForegroundRole) {
        return QIdentityProxyModel::data(index, role);
    }

    if (!QIdentityProxyModel::data(index, isExcludedRole_).toBool()) {
        return QIdentityProxyModel::data(index, role);
    }

    return QVariant::fromValue(semanticColors().muted);
}

} // namespace ui_shell
