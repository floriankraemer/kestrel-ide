#include "icon_decoration_proxy.h"

#include <QTreeView>
#include <QVariant>
#include <QWidget>

namespace ui_shell {

IconDecorationProxy::IconDecorationProxy(int iconKeyRole, int logicalPx, QObject *parent)
  : QIdentityProxyModel(parent)
  , m_iconKeyRole(iconKeyRole)
  , m_logicalPx(logicalPx)
{
}

QVariant IconDecorationProxy::data(const QModelIndex &index, int role) const
{
    if (role != Qt::DecorationRole) {
        return QIdentityProxyModel::data(index, role);
    }

    const QString key = QIdentityProxyModel::data(index, m_iconKeyRole).toString();
    if (key.isEmpty()) {
        return {};
    }
    return sharedIconCache().iconFor(key, m_logicalPx);
}

void IconDecorationProxy::clearIcons()
{
    sharedIconCache().clear();
}

void refreshTreeIcons(QWidget *projectTree)
{
    auto *tree = qobject_cast<QTreeView *>(projectTree);
    if (tree == nullptr) {
        return;
    }
    // The project tree chains a second proxy (VcsStatusColorProxy, R6) on
    // top of this one, so `tree->model()` is not always the
    // `IconDecorationProxy` itself any more — walk down through
    // `sourceModel()` until one is found or the chain ends.
    QAbstractItemModel *model = tree->model();
    while (model != nullptr) {
        if (auto *proxy = qobject_cast<IconDecorationProxy *>(model)) {
            proxy->clearIcons();
            break;
        }
        auto *identity = qobject_cast<QIdentityProxyModel *>(model);
        model = identity != nullptr ? identity->sourceModel() : nullptr;
    }
    tree->viewport()->update();
}

} // namespace ui_shell
