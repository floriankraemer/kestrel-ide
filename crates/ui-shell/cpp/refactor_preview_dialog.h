#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QCoreApplication>
#include <QDialog>
#include <QHash>
#include <QString>
#include <QStringList>

#include <functional>

class QCheckBox;
class QLabel;
class QSplitter;
class QTreeWidget;
class QTreeWidgetItem;
class QVBoxLayout;

namespace ui_shell {

class DiffView;

// The preview shown before a refactoring that reaches beyond the file the
// user is looking at.
//
// One row per file, one child row per edit, every file checkable. It decides
// nothing: whether a preview is needed at all is `lsp_core::EditPlan::
// touches_other_files`, and which sites of a name-based rename start ticked
// is `index_core::plan_index_rename`'s judgement. This class paints those
// answers and reports which files the user turned off.
//
// Modelled on SearchResultsPanel's Replace in Files tree, deliberately: the
// two gestures have the same shape (a list of pending writes, per-item opt
// out, a confirm), and a second idiom for it would be one more thing to
// learn.
class RefactorPreviewDialog : public QDialog
{
    // Not a `Q_OBJECT` (no moc target), so `tr()` would resolve in a Qt base
    // class's context; this keeps it in this class's own, where the .ts entries live.
    Q_DECLARE_TR_FUNCTIONS(RefactorPreviewDialog)

public:
    // One line of the preview. `line` is 0-based (the protocol's own
    // numbering) and shown 1-based, matching every other line number in the
    // application.
    struct Row
    {
        QString path;
        int line;
        QString detail;
        // False for a site a name-based rename found but cannot vouch for.
        bool certain;
        bool checked;
    };

    // Answers `path`'s before/after text and hunks for the diff panel
    // (F3-15), filling the out-parameters and returning whether it had one.
    // A row whose file the provider has nothing for (or a dialog built with
    // no provider at all) shows no diff panel — every call site decides for
    // itself whether it has a real per-file diff to offer; this class never
    // computes one.
    using DiffProvider = std::function<bool(const QString &path,
                                             QString &oldText,
                                             QString &newText,
                                             ::rust::Vec<FfiHunk> &hunks,
                                             ::rust::Vec<FfiInlineSpan> &spans)>;

    RefactorPreviewDialog(const QString &title,
                          const QString &explanation,
                          const QList<Row> &rows,
                          QWidget *parent,
                          DiffProvider diffProvider = {});

    // Files the user unticked. Excluded rather than included, because that
    // is what the bridge takes: the plan is the source of truth and the
    // dialog reports the subtractions from it.
    QStringList excludedPaths() const;

protected:
    // Marker-stream reporting only (e2e_mark.h): the base behaviour is
    // unchanged. Overridden rather than hooked onto accepted/rejected so a
    // close by the window manager is reported too.
    void done(int result) override;

private:
    void showDiffFor(const QString &path);

    QTreeWidget *tree_ = nullptr;
    QLabel *statusLabel_ = nullptr;
    QSplitter *splitter_ = nullptr;
    QVBoxLayout *diffLayout_ = nullptr;
    DiffView *diffView_ = nullptr;
    DiffProvider diffProvider_;
};

} // namespace ui_shell
