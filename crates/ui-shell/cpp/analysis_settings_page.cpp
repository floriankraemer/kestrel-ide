#include "analysis_settings_page.h"

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QCheckBox>
#include <QComboBox>
#include <QHash>
#include <QFontMetrics>
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

QString analyzerStatusText(const FfiAnalyzerRow &row)
{
    if (row.statusKind != FfiAnalyzerStatusKind::NeedsConfig) {
        return row.statusText;
    }
    return QString(row.configCommand).isEmpty()
             ? QObject::tr("%1 needs a %2").arg(QString(row.name), QString(row.configName))
             : QObject::tr("%1 needs a %2 — run `%3`")
                 .arg(QString(row.name), QString(row.configName), QString(row.configCommand));
}

QWidget *buildAnalysisSettingsPage(QWidget *parent, AnalysisEditor *editor,
                                   AnalysisService *analysisService)
{
    auto *page = new QWidget(parent);
    auto *layout = new QVBoxLayout(page);

    auto *tree = new QTreeWidget(page);
    tree->setColumnCount(4);
    tree->setHeaderLabels({QObject::tr("On"), QObject::tr("Analyzer"), QObject::tr("Run"),
                          QObject::tr("Status")});
    // Status holds the longest text (the detected command line), so it takes
    // the slack; the other columns fit their contents instead of splitting
    // the width and eliding Status at the dialog's default size.
    tree->header()->setSectionResizeMode(kOnColumn, QHeaderView::ResizeToContents);
    tree->header()->setSectionResizeMode(kNameColumn, QHeaderView::ResizeToContents);
    // The Run column holds combo boxes, which ResizeToContents does not see,
    // so it is sized from the widest combo once the rows exist.
    tree->header()->setSectionResizeMode(kTriggerColumn, QHeaderView::Fixed);
    tree->header()->setSectionResizeMode(kStatusColumn, QHeaderView::Stretch);
    tree->setTextElideMode(Qt::ElideMiddle);
    tree->setRootIsDecorated(false);
    layout->addWidget(tree);

    // A one-time snapshot of detection status, keyed by id — see the
    // header's note on why this page never re-detects anything itself.
    QHash<QString, QString> statusById;
    for (const FfiAnalyzerRow &row : analysisService->analyzerRows()) {
        statusById.insert(row.id, analyzerStatusText(row));
    }

    int triggerWidth = 0;
    for (const FfiAnalysisRow &row : editor->rows()) {
        auto *item = new QTreeWidgetItem(tree);
        item->setData(kNameColumn, kAnalyzerIdRole, row.id);
        item->setCheckState(kOnColumn, row.enabled ? Qt::Checked : Qt::Unchecked);
        item->setText(kNameColumn, row.name);
        item->setText(kStatusColumn, statusById.value(row.id));
        item->setToolTip(kStatusColumn, statusById.value(row.id));

        auto *triggerBox = new QComboBox(tree);
        triggerBox->addItem(QObject::tr("On Type"), QStringLiteral("on-type"));
        triggerBox->addItem(QObject::tr("On Save"), QStringLiteral("on-save"));
        triggerBox->addItem(QObject::tr("Manual"), QStringLiteral("manual"));
        const int index = triggerBox->findData(row.triggerId);
        triggerBox->setCurrentIndex(index >= 0 ? index : 0);
        tree->setItemWidget(item, kTriggerColumn, triggerBox);
        // The themed combo's size hint leaves out its own padding and arrow,
        // which clipped the last letter of "On Type"; measure the widest
        // entry and add room for both.
        const QFontMetrics metrics(triggerBox->font());
        int widest = 0;
        for (int i = 0; i < triggerBox->count(); ++i) {
            widest = qMax(widest, metrics.horizontalAdvance(triggerBox->itemText(i)));
        }
        triggerBox->setMinimumWidth(widest + 44);
        triggerWidth = qMax(triggerWidth, widest + 44 + 8);

        QObject::connect(triggerBox, &QComboBox::currentIndexChanged, editor,
                         [editor, id = row.id, triggerBox](int) {
                             editor->setTrigger(id, triggerBox->currentData().toString());
                         });
    }

    tree->setColumnWidth(kTriggerColumn, triggerWidth);

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
