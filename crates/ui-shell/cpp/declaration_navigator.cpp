#include "declaration_navigator.h"

#include "editor_tabs.h"
#include "symbol_kind_label.h"

#include <QAction>
#include <QCursor>
#include <QFileInfo>
#include <QMainWindow>
#include <QMenu>
#include <QPair>
#include <QStatusBar>
#include <utility>

namespace ui_shell {

DeclarationNavigator::DeclarationNavigator(LanguageService *languageService,
                                            SearchModel *searchModel, EditorTabs *editorTabs,
                                            QMainWindow *window)
  : QObject(window)
  , languageService_(languageService)
  , searchModel_(searchModel)
  , editorTabs_(editorTabs)
  , window_(window)
{
    connect(searchModel_, &SearchModel::declarationFound, this,
            [this](const FfiSymbolMatch &row) {
                candidates_.append(
                  Candidate{row.path, row.line, row.column,
                             row.has_kind ? symbolKindLabel(row.kind) : QString(),
                             row.container});
            });
    connect(searchModel_, &SearchModel::declarationFinished, this,
            &DeclarationNavigator::finish);
    connect(searchModel_, &SearchModel::declarationFailed, this,
            [this](const QString &message) {
                candidates_.clear();
                report(tr("Go to Declaration failed: %1").arg(message));
            });

    // L4: the language server's answer, when it had one. Targets carry
    // no kind or container — a server answers with places, not with the
    // index's symbol metadata.
    connect(languageService_, &LanguageService::definitionFound, this,
            [this](const FfiDefinition &target) {
                candidates_.append(
                  Candidate{target.path, target.line, target.column, QString(), QString()});
            });
    connect(languageService_, &LanguageService::definitionFinished, this, [this]() {
        // Project tier: several targets are several real answers, so
        // they are offered rather than silently reduced to the first.
        finish(FfiResolutionTier::Project, QString());
    });
    connect(languageService_, &LanguageService::definitionFallback, this,
            &DeclarationNavigator::askIndex);

    // C12: the server's answer was decompiled/generated source this IDE
    // cannot open yet (a non-file: URI) — refuse cleanly with a status-bar
    // message, the same channel "no declaration found" already uses, rather
    // than leaving whatever candidates_ held or attempting a jump.
    connect(languageService_, &LanguageService::definitionUnavailable, this,
            [this](const QString &message) {
                candidates_.clear();
                report(message);
            });
}

void DeclarationNavigator::resolveAt(int documentPosition)
{
    const QString path = editorTabs_->currentPath();
    if (path.isEmpty()) {
        return;
    }
    candidates_.clear();
    position_ = documentPosition;
    const QPair<quint32, quint32> at = editorTabs_->lspPositionAt(documentPosition);
    languageService_->resolveDefinition(path, at.first, at.second);
}

void DeclarationNavigator::resolveTypeAt(int documentPosition)
{
    const QString path = editorTabs_->currentPath();
    if (path.isEmpty()) {
        return;
    }
    candidates_.clear();
    position_ = documentPosition;
    const QPair<quint32, quint32> at = editorTabs_->lspPositionAt(documentPosition);
    languageService_->resolveTypeDefinition(path, at.first, at.second);
}

void DeclarationNavigator::askIndex()
{
    const QString path = editorTabs_->currentPath();
    if (path.isEmpty()) {
        return;
    }
    candidates_.clear();
    searchModel_->resolveDeclaration(path, editorTabs_->currentContent(),
                                      editorTabs_->byteOffsetAt(position_));
}

void DeclarationNavigator::finish(FfiResolutionTier tier, const QString &name)
{
    const QList<Candidate> candidates = std::move(candidates_);
    candidates_.clear();
    if (candidates.isEmpty()) {
        report(name.isEmpty() ? tr("No identifier under the caret.")
                              : tr("No declaration found for \"%1\".").arg(name));
        return;
    }
    // A local-file result is ranked, not merely listed: the first
    // candidate is the innermost binding that shadows the caret, so
    // offering a chooser would contradict the ranking that made it
    // first. Only project-tier ambiguity is genuine — same name,
    // unrelated symbols, nothing to prefer between them.
    if (candidates.size() == 1 || tier == FfiResolutionTier::LocalFile) {
        jumpTo(candidates.first());
        return;
    }
    chooseAmong(candidates, name);
}

void DeclarationNavigator::jumpTo(const Candidate &candidate)
{
    editorTabs_->openFileAtLine(candidate.path, static_cast<int>(candidate.line),
                                 static_cast<int>(candidate.column));
}

void DeclarationNavigator::chooseAmong(const QList<Candidate> &candidates, const QString &name)
{
    QMenu menu(window_);
    menu.setTitle(tr("Declarations of \"%1\"").arg(name));
    for (const Candidate &candidate : candidates) {
        const QString file = QFileInfo(candidate.path).fileName();
        QString label = tr("%1:%2").arg(file).arg(candidate.line);
        if (!candidate.container.isEmpty()) {
            label = tr("%1 in %2").arg(label, candidate.container);
        }
        if (!candidate.kind.isEmpty()) {
            label = tr("%1 (%2)").arg(label, candidate.kind);
        }
        QAction *action = menu.addAction(label);
        connect(action, &QAction::triggered, this,
                [this, candidate]() { jumpTo(candidate); });
    }
    menu.exec(QCursor::pos());
}

void DeclarationNavigator::report(const QString &message)
{
    window_->statusBar()->showMessage(message, 4000);
}
} // namespace ui_shell
