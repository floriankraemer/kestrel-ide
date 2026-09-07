#include "commit_detail_view.h"

#include "diff_view.h"
#include "e2e_mark.h"

#include <QDateTime>
#include <QLabel>
#include <QLocale>
#include <QPlainTextEdit>
#include <QScrollArea>
#include <QVBoxLayout>

namespace ui_shell {

namespace {
// Each file's `DiffView` scrolls its own two panes; bounding its height
// keeps one huge file from pushing every other changed file below the
// fold — the outer scroll area is what moves between files.
constexpr int kDiffViewHeight = 320;

QString formatChangeKind(FfiChangeKind kind)
{
    switch (kind) {
    case FfiChangeKind::Added:
        return QObject::tr("added");
    case FfiChangeKind::Deleted:
        return QObject::tr("deleted");
    case FfiChangeKind::TypeChanged:
        return QObject::tr("type changed");
    case FfiChangeKind::Modified:
    case FfiChangeKind::None:
    case FfiChangeKind::Untracked:
    default:
        return QObject::tr("modified");
    }
}
} // namespace

CommitDetailView::CommitDetailView(VcsService *vcsService, const QString &commitId,
                                    QWidget *parent)
  : QWidget(parent), vcsService_(vcsService), commitId_(commitId)
{
    headerLabel_ = new QLabel(tr("Loading…"), this);
    headerLabel_->setWordWrap(true);
    headerLabel_->setTextFormat(Qt::RichText);

    bodyEdit_ = new QPlainTextEdit(this);
    bodyEdit_->setReadOnly(true);
    bodyEdit_->setMaximumHeight(160);

    auto *diffsContainer = new QWidget(this);
    diffsLayout_ = new QVBoxLayout(diffsContainer);
    diffsLayout_->setContentsMargins(0, 0, 0, 0);
    diffsLayout_->addStretch(1);

    auto *scrollArea = new QScrollArea(this);
    scrollArea->setWidgetResizable(true);
    scrollArea->setWidget(diffsContainer);

    auto *layout = new QVBoxLayout(this);
    layout->addWidget(headerLabel_);
    layout->addWidget(bodyEdit_);
    layout->addWidget(scrollArea, 1);

    connect(vcsService_, &VcsService::commitDetailReady, this,
            &CommitDetailView::onCommitDetailReady);
    connect(vcsService_, &VcsService::commitFileDiffReady, this,
            &CommitDetailView::onCommitFileDiffReady);
    vcsService_->requestCommitDetail(commitId_);
}

void CommitDetailView::onCommitDetailReady(const QString &id)
{
    if (id != commitId_) {
        // Another tab's commit answered — every `CommitDetailView` in the
        // dock listens to the same signal.
        return;
    }
    const FfiCommitDetail detail = vcsService_->commitDetail(commitId_);
    const QLocale locale;
    const QDateTime authorTime = QDateTime::fromSecsSinceEpoch(detail.author_time);
    const QDateTime committerTime = QDateTime::fromSecsSinceEpoch(detail.committer_time);
    QString header = tr("<b>%1</b><br>Commit %2<br>Author: %3 &lt;%4&gt;, %5")
                        .arg(QString(detail.summary).toHtmlEscaped(), QString(detail.id),
                             QString(detail.author_name).toHtmlEscaped(),
                             QString(detail.author_email).toHtmlEscaped(),
                             locale.toString(authorTime, QLocale::LongFormat));
    if (QString(detail.committer_email) != QString(detail.author_email)
        || QString(detail.committer_name) != QString(detail.author_name)) {
        header += tr("<br>Committer: %1 &lt;%2&gt;, %3")
                    .arg(QString(detail.committer_name).toHtmlEscaped(),
                         QString(detail.committer_email).toHtmlEscaped(),
                         locale.toString(committerTime, QLocale::LongFormat));
    }
    headerLabel_->setText(header);
    bodyEdit_->setPlainText(QString(detail.body));

    for (const FfiChangedCommitFile &file : vcsService_->changedCommitFiles(commitId_)) {
        const QString path = QString(file.path);
        auto *container = new QWidget();
        auto *containerLayout = new QVBoxLayout(container);
        containerLayout->setContentsMargins(0, 0, 0, 8);
        auto *fileLabel =
          new QLabel(tr("%1 (%2)").arg(path, formatChangeKind(file.change)), container);
        containerLayout->addWidget(fileLabel);
        auto *placeholder = new QLabel(tr("Loading diff…"), container);
        containerLayout->addWidget(placeholder);
        // Before the stretch the constructor added, so files stack from
        // the top with the stretch still soaking up leftover space below.
        diffsLayout_->insertWidget(diffsLayout_->count() - 1, container);
        diffContainersByPath_.insert(path, container);
        vcsService_->requestCommitFileDiff(commitId_, path);
    }

    e2eMark(QStringLiteral("{\"ev\":\"commit_detail_ready\",\"commit\":%1,\"files\":%2}")
              .arg(e2eJson(commitId_))
              .arg(diffContainersByPath_.size()));
}

void CommitDetailView::onCommitFileDiffReady(const QString &id, const QString &path)
{
    if (id != commitId_) {
        return;
    }
    QWidget *container = diffContainersByPath_.value(path);
    if (!container) {
        return;
    }
    const FfiFileDiff diff = vcsService_->commitFileDiff(commitId_, path);
    const ::rust::Vec<FfiHunk> hunks = vcsService_->commitFileDiffHunks(commitId_, path);
    auto *diffView = new DiffView(QString(diff.old_text), QString(diff.new_text), hunks,
                                  ::rust::Vec<FfiInlineSpan>(), path, container);
    diffView->setMinimumHeight(kDiffViewHeight);
    diffView->setMaximumHeight(kDiffViewHeight);
    // Replace the "Loading diff…" placeholder — always the last (second)
    // child the constructor added, a file label being the first.
    auto *containerLayout = qobject_cast<QVBoxLayout *>(container->layout());
    QLayoutItem *old = containerLayout->takeAt(1);
    delete old->widget();
    delete old;
    containerLayout->addWidget(diffView);
}

} // namespace ui_shell
