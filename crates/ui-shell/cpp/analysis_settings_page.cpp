#include "analysis_settings_page.h"

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QCheckBox>
#include <QComboBox>
#include <QHash>
#include <QHeaderView>
#include <QTreeWidget>
#include <QTreeWidgetItem>
#include <QVBoxLayout>
#include <QWidget>

namespace ui_shell {

namespace {

constexpr int kOnColumn = 0;
constexpr int kNameColumn = 1;
constexpr int kTriggerColumn = 2;
constexpr int kStatusColumn = 3;
constexpr int kAnalyzerIdRole = Qt::UserRole;

} // namespace

QWidget *buildAnalysisSettingsPage(QWidget *parent, AnalysisEditor *editor,
                                   AnalysisService *analysisService)
{
    auto *page = new QWidget(parent);
    auto *layout = new QVBoxLayout(page);

    auto *tree = new QTreeWidget(page);
    tree->setColumnCount(4);
    tree->setHeaderLabels({QObject::tr("On"), QObject::tr("Analyzer"), QObject::tr("Run"),
                          QObject::tr("Status")});
    tree->header()->setSectionResizeMode(kNameColumn, QHeaderView::Stretch);
    tree->setRootIsDecorated(false);
    layout->addWidget(tree);

    // A one-time snapshot of detection status, keyed by id — see the
    // header's note on why this page never re-detects anything itself.
    QHash<QString, QString> statusById;
    for (const FfiAnalyzerRow &row : analysisService->analyzerRows()) {
        statusById.insert(row.id, row.statusText);
    }

    for (const FfiAnalysisRow &row : editor->rows()) {
        auto *item = new QTreeWidgetItem(tree);
        item->setData(kNameColumn, kAnalyzerIdRole, row.id);
        item->setCheckState(kOnColumn, row.enabled ? Qt::Checked : Qt::Unchecked);
        item->setText(kNameColumn, row.name);
        item->setText(kStatusColumn, statusById.value(row.id));

        auto *triggerBox = new QComboBox(tree);
        triggerBox->addItem(QObject::tr("On Type"), QStringLiteral("on-type"));
        triggerBox->addItem(QObject::tr("On Save"), QStringLiteral("on-save"));
        triggerBox->addItem(QObject::tr("Manual"), QStringLiteral("manual"));
        const int index = triggerBox->findData(row.triggerId);
        triggerBox->setCurrentIndex(index >= 0 ? index : 0);
        tree->setItemWidget(item, kTriggerColumn, triggerBox);

        QObject::connect(triggerBox, &QComboBox::currentIndexChanged, editor,
                         [editor, id = row.id, triggerBox](int) {
                             editor->setTrigger(id, triggerBox->currentData().toString());
                         });
    }

    QObject::connect(tree, &QTreeWidget::itemChanged, editor,
                     [editor](QTreeWidgetItem *item, int column) {
                         if (column != kOnColumn) {
                             return;
                         }
                         const QString id = item->data(kNameColumn, kAnalyzerIdRole).toString();
                         editor->setEnabled(id, item->checkState(kOnColumn) == Qt::Checked);
                     });

    return page;
}

} // namespace ui_shell
