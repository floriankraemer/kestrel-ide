#include "refactor_preview_dialog.h"

#include "diff_view.h"
#include "e2e_mark.h"

#include <QDialogButtonBox>
#include <QFileInfo>
#include <QHeaderView>
#include <QLabel>
#include <QPushButton>
#include <QSplitter>
#include <QTimer>
#include <QTreeWidget>
#include <QVBoxLayout>

namespace ui_shell {
namespace {

constexpr int kPathRole = Qt::UserRole + 1;

} // namespace

RefactorPreviewDialog::RefactorPreviewDialog(const QString &title,
                                              const QString &explanation,
                                              const QList<Row> &rows,
                                              QWidget *parent,
                                              DiffProvider diffProvider)
  : QDialog(parent)
  , diffProvider_(std::move(diffProvider))
{
    setWindowTitle(title);
    setModal(true);
    resize(diffProvider_ ? 1000 : 720, 560);

    auto *layout = new QVBoxLayout(this);
    auto *header = new QLabel(explanation, this);
    header->setWordWrap(true);
    layout->addWidget(header);

    tree_ = new QTreeWidget(this);
    tree_->setColumnCount(1);
    tree_->setHeaderHidden(true);
    tree_->setUniformRowHeights(true);

    if (diffProvider_) {
        // A file selected in the tree shows its diff beside it — the panel
        // exists only when the caller has a real per-file diff to offer.
        splitter_ = new QSplitter(this);
        splitter_->addWidget(tree_);
        auto *diffHost = new QWidget(splitter_);
        diffLayout_ = new QVBoxLayout(diffHost);
        diffLayout_->setContentsMargins(0, 0, 0, 0);
        splitter_->addWidget(diffHost);
        splitter_->setStretchFactor(0, 1);
        splitter_->setStretchFactor(1, 2);
        layout->addWidget(splitter_, 1);

        connect(tree_, &QTreeWidget::currentItemChanged, this,
                [this](QTreeWidgetItem *current, QTreeWidgetItem *) {
                    if (current) {
                        showDiffFor(current->data(0, kPathRole).toString());
                    }
                });
    } else {
        layout->addWidget(tree_, 1);
    }

    QHash<QString, QTreeWidgetItem *> groups;
    for (const Row &row : rows) {
        QTreeWidgetItem *group = groups.value(row.path);
        if (!group) {
            group = new QTreeWidgetItem(tree_);
            group->setText(0, QFileInfo(row.path).fileName());
            group->setToolTip(0, row.path);
            group->setData(0, kPathRole, row.path);
            group->setFlags(group->flags() | Qt::ItemIsUserCheckable);
            // A file starts ticked unless the plan says otherwise; the first
            // row decides, and a file whose rows disagree keeps the ticked
            // state, since a partially-certain file is still worth offering.
            group->setCheckState(0, row.checked ? Qt::Checked : Qt::Unchecked);
            group->setExpanded(true);
            groups.insert(row.path, group);
        } else if (row.checked && group->checkState(0) == Qt::Unchecked) {
            group->setCheckState(0, Qt::Checked);
        }

        auto *item = new QTreeWidgetItem(group);
        item->setText(0, tr("Line %1: %2").arg(row.line + 1).arg(row.detail));
        item->setData(0, kPathRole, row.path);
        if (!row.certain) {
            item->setText(0, tr("Line %1: %2  — name match only")
                                .arg(row.line + 1)
                                .arg(row.detail));
        }
    }

    statusLabel_ = new QLabel(tr("%n change(s) in %1 file(s).", "", rows.size())
                                 .arg(groups.size()),
                              this);
    layout->addWidget(statusLabel_);

    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, this);
    buttons->button(QDialogButtonBox::Ok)->setText(tr("Apply"));
    connect(buttons, &QDialogButtonBox::accepted, this, &QDialog::accept);
    connect(buttons, &QDialogButtonBox::rejected, this, &QDialog::reject);
    layout->addWidget(buttons);

    e2eMark(QStringLiteral("{\"ev\":\"preview_rows\",\"count\":%1,\"files\":%2}")
              .arg(rows.size())
              .arg(groups.size()));
    // #346: `dialog_shown` fires once `exec()`'s modal loop is running and
    // the dialog is mapped, the convention `settings_dialog.cpp` explains —
    // marked from this constructor it came before the dialog existed on
    // screen, and a flow's `Return` raced it. `window` is the X window id,
    // because under bare Xvfb nothing hands a new toplevel the input focus:
    // a key sent while the main window still holds it is dropped by the
    // modal block, so a flow has to focus this window itself.
    QTimer::singleShot(0, this, [this]() {
        e2eMark(QStringLiteral("{\"ev\":\"dialog_shown\",\"name\":\"refactor_preview\","
                                "\"window\":\"%1\"}")
                  .arg(winId()));
    });
}

void RefactorPreviewDialog::done(int result)
{
    QDialog::done(result);
    e2eMark(QStringLiteral("{\"ev\":\"dialog_closed\",\"name\":\"refactor_preview\","
                            "\"accepted\":%1}")
              .arg(result == QDialog::Accepted ? QLatin1String("true") : QLatin1String("false")));
}

void RefactorPreviewDialog::showDiffFor(const QString &path)
{
    if (!diffProvider_ || !diffLayout_) {
        return;
    }
    QString oldText;
    QString newText;
    ::rust::Vec<FfiHunk> hunks;
    ::rust::Vec<FfiInlineSpan> spans;
    if (path.isEmpty() || !diffProvider_(path, oldText, newText, hunks, spans)) {
        delete diffView_;
        diffView_ = nullptr;
        return;
    }
    delete diffView_;
    // No language id: the dialog does not track which language each row's
    // file is, and `DiffView` treats an empty one as plain text (F3-13).
    diffView_ = new DiffView(oldText, newText, hunks, spans, QString());
    diffLayout_->addWidget(diffView_);
}

QStringList RefactorPreviewDialog::excludedPaths() const
{
    QStringList excluded;
    for (int i = 0; i < tree_->topLevelItemCount(); ++i) {
        const QTreeWidgetItem *group = tree_->topLevelItem(i);
        if (group->checkState(0) != Qt::Checked) {
            excluded.append(group->data(0, kPathRole).toString());
        }
    }
    return excluded;
}

} // namespace ui_shell
