#pragma once

#include "diff_selections.h"
#include "minimap.h"
#include "vcs_gutter.h"

#include <QColor>
#include <QSet>
#include <QHash>

#include <QPlainTextEdit>
#include <QSize>
#include <QPair>
#include <QString>
#include <QVector>
#include <QWidget>

#include <functional>

class QCompleter;
class QContextMenuEvent;
class QEvent;
class QFocusEvent;
class QKeyEvent;
class QMimeData;
class QMouseEvent;
class QStandardItemModel;
class QMenu;
class QPaintEvent;
class QResizeEvent;

namespace ui_shell {

class LineNumberArea;

// A foldable region (Task C), view-local: [startBlock, endBlock] are
// 0-based QTextBlock numbers (inclusive), converted by SyntaxHighlighter
// from syntax_core::FoldRange byte offsets via its existing UTF-8<->UTF-16
// offset table. Kept as a plain struct (not the FFI type) so this widget
// stays decoupled from the cxx-qt-generated header — its only job is
// rendering markers and hiding/showing blocks, never deciding what's
// foldable (that's Qt-free Rust).
struct FoldRange
{
    // Body's start block (the `{`/opening-construct line) and end block —
    // this is the range that actually gets hidden on collapse, via
    // setBlocksVisible(startBlock, endBlock, ...): the marker/click anchor
    // below can sit earlier, on the declaration line, while the hidden span
    // still starts right after the body opens.
    int startBlock;
    int endBlock;
    // Line the fold marker is drawn/clicked on: the declaration that owns
    // this block (`fn foo(...)`, `class Foo`, ...) rather than `startBlock`
    // itself, so a multi-line signature between the declaration and the
    // `{` stays visible when collapsed. Equal to startBlock when the two
    // coincide (the common case: `fn foo() {` on one line).
    int anchorBlock;

    bool operator==(const FoldRange &other) const
    {
        return startBlock == other.startBlock && endBlock == other.endBlock
            && anchorBlock == other.anchorBlock;
    }
};


// One line's blame annotation (F3-18), view-local: `block` is the 0-based
// `QTextBlock` the line's `git blame` entry describes, `text` is the
// pre-formatted summary this widget paints verbatim ("author, summary" or
// similar) — deciding what that string says is EditorTabs's job (it reads
// `FfiBlameLine`), not this widget's, the same split `ChangeMarker` draws.
struct BlameAnnotation
{
    int block;
    QString text;
};

// One diagnostic's underline, view-local: [start, end) document (UTF-16)
// positions plus the colour its severity gets. Converted from the FFI's
// line/character pairs by whoever owns the mapping (main_window.cpp), so this
// widget stays decoupled from the cxx-qt-generated header — same arrangement
// as FoldRange above.
struct DiagnosticSpan
{
    int start;
    int end;
    QColor color;
};

// One occurrence of the symbol under the caret (F2-11), view-local for the
// same reason DiagnosticSpan is: [start, end) document (UTF-16) positions,
// converted from the FFI type by whoever owns the mapping.
// `document_highlight::HighlightKind::Write` paints differently from a
// plain read/text occurrence — the distinction the server drew, not
// something guessed here.
struct OccurrenceSpan
{
    int start;
    int end;
    bool isWrite;

    bool operator==(const OccurrenceSpan &other) const
    {
        return start == other.start && end == other.end && isWrite == other.isWrite;
    }
};

// One inlay hint (F2-11), view-local for the same reason. `position` is
// where the label is inserted; `paddingLeft`/`paddingRight` are the
// server's own spacing request (`lsp_core::InlayHint`'s fields, carried
// through rather than guessed from `kind` — a parameter hint and a type
// hint pad differently and the server already said which).
struct InlayHintSpan
{
    int position;
    QString label;
    bool paddingLeft;
    bool paddingRight;
};

// One code lens (C10-followup), 1:1 with `FfiCodeLens` — `line` is a
// 0-based block number, matching every other LSP-sourced line this widget
// already takes (see InlayHintSpan's own `position`, resolved the same
// way). `clickable` is false for a lens the server hasn't resolved a
// command for yet (`lsp_core::CodeLensItem::needs_resolve`); it still
// paints, since its range is real even before its label is.
struct CodeLensSpan
{
    int line;
    QString label;
    bool clickable;
};

// One line's inline debug values (D3-7), view-local like every other span
// here. `line` is a 0-based block number, converted from `DebugService`'s
// 1-based line by whoever owns the mapping (EditorTabs), exactly as
// CodeLensSpan is.
struct InlineValueSpan
{
    int line;
    QString text;

    bool operator==(const InlineValueSpan &other) const
    {
        return line == other.line && text == other.text;
    }
};

// One classified space/tab character (show-whitespace-characters task),
// view-local for the same reason FoldRange and DiagnosticSpan are:
// converted from FfiWhitespaceSpan by whoever owns the mapping (EditorTabs),
// so this widget stays decoupled from the cxx-qt generated header. `line`/
// `column` are relative to whatever multi-line text the classifier was
// asked about — this widget's own paintEvent, which only ever asks about
// its currently visible blocks — not absolute document positions.
// `category` mirrors `editor_core::whitespace::WhitespaceCategory`: 0 =
// leading, 1 = inner, 2 = trailing.
struct WhitespaceSpan
{
    int line;
    int column;
    bool isTab;
    int category;
};

// JetBrains-style "show whitespace characters" (show-whitespace-characters
// task): the master toggle plus which of leading/inner/trailing paint when
// it is on, and the independent end-of-line marker toggle. 1:1 with
// `FfiWhitespaceOptions`, but view-local like every other struct here.
struct WhitespaceOptions
{
    bool enabled = false;
    bool leading = false;
    bool inner = false;
    bool trailing = false;
    bool eolMarkers = false;
};

// One caret that is not the widget's own (F1-15), view-local for the same
// reason FoldRange and DiagnosticSpan are: [anchor, head] document (UTF-16)
// positions, converted from the FFI type by whoever owns the mapping.
//
// The primary caret is never in this list — it stays the widget's
// QTextCursor, so scrolling, Find, the status bar and Qt's own selection
// handling keep working untouched. `editor_ops` owns the whole set and
// decides where every caret lands; this widget paints what it is given.
struct SecondaryCaret
{
    int anchor;
    int head;

    bool operator==(const SecondaryCaret &other) const
    {
        return anchor == other.anchor && head == other.head;
    }
};

// One completion candidate as the popup shows it (Task L5), view-local for
// the same reason FoldRange and DiagnosticSpan are: converted from the FFI
// type by main_window.cpp, so this widget stays decoupled from the cxx-qt
// generated header. Nothing here is decided by the view — `insert` is the
// text the server said to type (`lsp_core::completion` already resolved
// textEdit/insertText/label precedence and flattened any snippet), and the
// order of the vector is the server's `sortText` order.
struct CompletionEntry
{
    QString label;
    QString kind;
    QString detail;
    QString documentation;
    QString insert;
    // When true, the server named the span to replace, in protocol units:
    // 0-based lines, UTF-16 characters. When false, the word the caret is
    // in is replaced instead.
    bool hasRange;
    int startLine;
    int startCharacter;
    int endLine;
    int endCharacter;
    // How many UTF-16 characters before the caret the typed word occupies —
    // what gets replaced when `hasRange` is false. Derived from the same
    // text the request was made about, in `lsp_core::completion`.
    int prefixLength;
    // C7: the server's own item, as opaque JSON text, carried only so it can
    // be handed back for `completionItem/resolve` on accept or on preview.
    // This widget never reads it.
    QString resolveData;
};

// Line-number gutter (Qt's classic Code Editor Example pattern). Q_OBJECT is
// required here for the block-count/scroll signals below to reach private
// slots — this is the crate's first hand-written (non-cxx-qt-generated)
// QObject, so it is also the first header build.rs runs moc over.
class CodeEditor : public QPlainTextEdit
{
    Q_OBJECT

public:
    explicit CodeEditor(QWidget *parent = nullptr);

    void lineNumberAreaPaintEvent(QPaintEvent *event);
    void lineNumberAreaMousePressEvent(QMouseEvent *event);
    // Right-click anywhere in the gutter: a small "Collapse All"/"Expand
    // All" menu, independent of which (if any) fold marker sits under the
    // pointer.
    void lineNumberAreaContextMenuEvent(QContextMenuEvent *event);
    int lineNumberAreaWidth() const;
    // R1-7: the Run-icon column's width — `kRunMarkerWidth` when this file
    // is runnable, otherwise zero.
    int runMarkerWidth() const;

    // Task C: called by SyntaxHighlighter whenever its incremental tree
    // updates (the same revision-change hook that already drives
    // highlighting — no separate invalidation path). Collapsed/expanded
    // state is pure view state, kept only in `collapsedRanges_` below —
    // not threaded through app-core/TabId, not persisted across sessions.
    void setFoldRanges(const QVector<FoldRange> &ranges);

    // Expands any collapsed fold hiding `blockNumber`, so a jump (Go to
    // Line, Find in Files, Go to Symbol) can't park the cursor on an
    // invisible line. Blocks nothing when the line is already visible.
    void ensureBlockVisible(int blockNumber);

    // Collapse/expand every fold in this editor (code.collapseAll /
    // code.expandAll). Reuses toggleFold's collapse/expand mechanics per
    // range, rather than duplicating the visibility bookkeeping, so
    // collapsedRanges_ and setFoldRanges's self-healing stay correct.
    void collapseAll();
    void expandAll();

    // Find (F3): the spans the find bar wants painted, as [start, end)
    // document positions. Purely decorative — the widget neither computes
    // them (editor_core::search does) nor tracks which one is current
    // beyond `currentMatch`, an index into `matches` or -1 for none.
    void setMatchSelections(const QVector<QPair<int, int>> &matches, int currentMatch);


    // Task L2: the diagnostic squiggles for this editor's file.
    //
    // Deliberately an extra-selection layer rather than formats pushed into
    // SyntaxHighlighter: a QSyntaxHighlighter owns the character formats of
    // every block it touches and rewrites them on each rehighlight, so a
    // diagnostic underline living there would be erased by the next keystroke
    // (or would have to be merged into every token format, coupling the two).
    // Extra selections are composited on top of the highlighter's formats by
    // QPlainTextEdit itself, arrive and disappear asynchronously with the
    // server, and cost no reparse — which is exactly the lifetime a
    // diagnostic has.
    void setDiagnosticSpans(const QVector<DiagnosticSpan> &spans);

    // F2-11: the occurrences of the symbol under the caret, same
    // extra-selection lifetime as diagnostics — asked again on every caret
    // settle, painted until the next answer replaces them.
    void setOccurrenceSpans(const QVector<OccurrenceSpan> &spans);

    // The diff viewer's line backgrounds and inline spans while this editor
    // is the right pane of the HEAD-vs-working-tree window — same
    // extra-selection lifetime as occurrences, cleared with two empty
    // vectors when the window hands the editor back.
    void setDiffSelections(const QVector<DiffLineBackground> &backgrounds,
                           const QVector<DiffInlineSpan> &spans);

    // F2-11: inlay hints for the visible range, and the settings toggle
    // that decides whether `paintEvent` draws them at all — off by default
    // (`code.toggleInlayHints`), since a hint is text the server invented,
    // not text in the file.
    void setInlayHints(const QVector<InlayHintSpan> &hints);
    void setInlayHintsEnabled(bool enabled);
    bool inlayHintsEnabled() const { return inlayHintsEnabled_; }

    // C10-followup: the code lens strip for this document's whole visible
    // range. Always on, unlike inlay hints — a lens is a fetched fact
    // ("3 references", "Run Test"), not a guess the editor is inventing
    // inline with the code.
    void setCodeLenses(const QVector<CodeLensSpan> &lenses);

    // D3-7: values from the stopped frame, painted after each line's text.
    // Empty whenever nothing is suspended in this file, which is what makes
    // "is the debugger stopped here" a question this widget never asks.
    void setInlineValues(const QVector<InlineValueSpan> &values);

    // Show-whitespace-characters task: what to paint. `paintEvent` re-asks
    // `whitespaceClassifier_` for the visible blocks whenever the document
    // revision or the visible block range changes — nothing to invalidate
    // here, setting new options just changes what the next paint filters
    // in or draws.
    void setWhitespaceOptions(const WhitespaceOptions &options);
    const WhitespaceOptions &whitespaceOptions() const { return whitespaceOptions_; }

    // Editor minimap (issue #199): which overlays paint, and the strip's
    // own opinion of how much viewport margin it needs — `0` when disabled,
    // so a file whose minimap is off keeps exactly the layout it had before
    // this feature existed.
    void setMinimapOptions(const MinimapOptions &options);
    int minimapWidth() const;

    // Read-only views the minimap's overlays paint from — the widget never
    // computes any of these itself, it only mirrors what is already stored
    // for the gutter's own painting.
    const QHash<int, ChangeMarker> &changeMarkers() const { return changeMarkers_; }
    const QVector<DiagnosticSpan> &diagnosticSpans() const { return diagnosticSpans_; }
    const QVector<QPair<int, int>> &matchSelections() const { return matchSelections_; }
    const QSet<int> &breakpointLines() const { return breakpointLines_; }

    // Classifies one multi-line slice of text (the widget's own currently
    // visible blocks, joined with '\n') into leading/inner/trailing
    // space-and-tab spans. Set once, at tab-open time, by whoever owns the
    // FFI seam (EditorTabs) — kept as a callback rather than a direct
    // `ui-shell/src/bridge` include so this header stays decoupled from the
    // cxx-qt generated one, the same reason WhitespaceSpan above is a
    // plain struct and not FfiWhitespaceSpan.
    using WhitespaceClassifier = std::function<QVector<WhitespaceSpan>(const QString &text)>;
    void setWhitespaceClassifier(WhitespaceClassifier classifier);

    // The tab width (in columns) a tab character advances to, resolved for
    // this tab's language. Recomputes `setTabStopDistance` from the
    // current font immediately, and `changeEvent` recomputes it again on
    // every later font change, so a rendered tab glyph always ends where
    // the tab actually ends.
    void setEditorTabWidth(int columns);

    // L5: show these candidates, or hide the popup when the vector is
    // empty. Called with whatever `LanguageService::completionItems` last
    // returned — this widget neither filters nor orders (that is
    // `lsp_core::completion`), it only paints and inserts.
    void showCompletions(const QVector<CompletionEntry> &items);

    // Re-asks for the candidates matching whatever word the caret is in
    // now, by emitting completionFilterChanged. Called when an answer
    // arrives and after every keystroke the popup survives.
    void refreshCompletions();

    // C7: a preview resolution for the currently highlighted row landed —
    // replace its tooltip with the resolved detail/documentation. A blank
    // `detail` and `documentation` (server had nothing further to say, or
    // never offered resolve at all) leaves the row's initial tooltip as it
    // was, painted from the unresolved item in showCompletions().
    void updateCompletionPreview(const QString &detail, const QString &documentation);

    // S2: explicit override for the current-line band, "#rrggbb" or empty
    // for "derive it from the editor palette" (the same empty-means-theme
    // contract the editor background/foreground overrides use).
    void setCurrentLineColor(const QString &hex);

    // F1-15: the carets other than this widget's own, painted as solid bars
    // and (where they select) as extra selections. Pushed in after every
    // operation, exactly like setFoldRanges and setDiagnosticSpans — the
    // widget never computes a caret position itself.
    void setSecondaryCarets(const QVector<SecondaryCaret> &carets);

    // Whether a keystroke is a multi-caret operation. A branch about which
    // code path runs, not about what an edit means, which is why it may
    // live here.
    bool hasSecondaryCarets() const { return !secondaryCarets_.isEmpty(); }

    // F3-16: the gutter's change markers against `HEAD`, pushed in by
    // EditorTabs whenever `VcsService::hunksChanged` answers for this
    // file — same "widget never computes it" arrangement as
    // setFoldRanges/setDiagnosticSpans.
    void setChangeMarkers(const QVector<ChangeMarker> &markers);

    // D2-5: which lines of this file have a breakpoint, and where execution
    // is currently suspended (-1 for "not in this file"). Both are pushed in
    // by EditorTabs from `DebugService`; the widget decides neither.
    void setBreakpointLines(const QSet<int> &lines);
    void setExecutionLine(int blockNumber);

    // R1-7: whether this file has a run target, decided by
    // `RunService::canRunFile` and pushed in by EditorTabs — the gutter
    // shows IntelliJ's Run icon on the first line when it does. The widget
    // never asks what makes a file runnable.
    void setRunnable(bool runnable);
    bool runnable() const { return runnable_; }

    // F3-18: per-line blame text, off by default (vcs.annotate toggles it).
    // Replaces whatever was set before — the caller re-sends the whole file's
    // annotations on every `blameReady`, same as setChangeMarkers.
    void setBlameAnnotations(const QVector<BlameAnnotation> &annotations);
    void setBlameEnabled(bool enabled);
    bool blameEnabled() const { return blameEnabled_; }

signals:
    // N7: Ctrl+Click landed on an identifier-shaped word. `position` is a
    // document (UTF-16) position inside that word; converting it to the
    // UTF-8 byte offset the index speaks, and deciding what the word
    // resolves to, both happen outside this widget — it only reports the
    // gesture.
    void declarationRequested(int position);

    // L3: the pointer rested over an identifier long enough for Qt to ask
    // for a tooltip. `position` is a document (UTF-16) position inside that
    // word; what it means, and whether the answer is still wanted when it
    // arrives, are decided outside this widget (`lsp_core::hover`).
    void hoverRequested(int position);

    // The pointer moved on or left: an answer to the last hoverRequested
    // must not be shown any more.
    void hoverCanceled();

    // L5: something happened that might want completions — a keystroke, or
    // Ctrl+Space (`explicitRequest`). `position` is a document (UTF-16)
    // position at the caret and `textBeforeCursor` is the current line up
    // to it; whether that is worth a request at all is decided in
    // `lsp_core::completion`, not here, so this fires on every keystroke.
    void completionRequested(int position, const QString &textBeforeCursor, bool explicitRequest);

    // L5: the caret moved within the word being completed, so the visible
    // candidates are stale. Carries the current line up to the caret — the
    // word inside it is picked out by `lsp_core::completion`, not here —
    // and the narrowed list comes back via showCompletions().
    void completionFilterChanged(const QString &textBeforeCursor);

    // L5: the popup closed, or the caret left the word — nothing in flight
    // is wanted.
    void completionCanceled();

    // C7: the popup's selection moved to `resolveData`'s item — ask for its
    // documentation/detail via `completionItem/resolve`, when the server
    // offers it at all (that check, and cancelling a stale request, are
    // `LanguageService::resolveCompletionPreview`'s job, not this widget's).
    void completionPreviewRequested(const QString &resolveData);

    // F0-18: the user accepted `entry`. What span the insertion replaces is
    // `lsp_core::completion`'s call, so this widget reports the choice and
    // lets the splice come back as edits rather than typing the text itself.
    void completionChosen(const CompletionEntry &entry);

    // The right-click menu has been built with Qt's standard entries and is
    // about to be shown: whoever wants to add to it does so now. The menu
    // is owned by this widget and deleted after it closes, so a receiver
    // must only append actions, never keep the pointer.
    //
    // A signal rather than a list this widget assembles, because what
    // belongs in it (refactorings, navigation) is a window-level question
    // and this widget knows nothing about either.
    void contextMenuAboutToShow(QMenu *menu);

    // F1-15: a keystroke that has to be applied at every caret. The widget
    // does not touch the document for these — it reports the gesture, and
    // the transaction `editor_ops` computes comes back as one splice, which
    // is what makes a 200-caret keystroke one Ctrl+Z (ADR-0023).
    void multiCaretTyped(const QString &text);
    void multiCaretBackspace();
    void multiCaretDelete();
    void multiCaretNewline();

    // F1-8: Ctrl+V (or a middle-click paste). `insertFromMimeData` is the
    // one correct override point regardless of how many carets there are —
    // it is called for every route text can enter the document from the
    // clipboard, not just the shortcut.
    void pasteRequested(const QString &text);

    // Alt+Click: one more caret at this document position.
    void caretAddRequested(int position);

    // Alt+Shift+drag: the two document positions the drag spans. What that
    // means in visual columns, and how ragged lines are treated, is
    // `editor_core::selection::column_block`'s answer, not this widget's.
    void columnSelectRequested(int anchor, int head);

    // Esc, a plain click, or any key this version does not treat as a
    // multi-caret operation: back to one caret.
    void secondaryCaretsDropped();

    // F3-16: a click landed on a change marker in the gutter's strip.
    // `hunkIndex` is the marker's own — what a hunk-revert/stage/show-diff
    // request needs — and `globalPos` is where to show the popup. Deciding
    // what the popup offers, and performing any of the three, is
    // EditorTabs's job; this widget only reports the gesture.
    void changeMarkerClicked(int hunkIndex, const QPoint &globalPos);

    // D2-5: the breakpoint column was clicked on this line (0-based block).
    // Whether that adds or removes one is `DebugService`'s answer.
    void breakpointToggled(int blockNumber);

    // R1-7: the gutter's Run icon was clicked. What that runs is
    // EditorTabs's business, via `RunService::runContext`.
    void runRequested();
    // C10-followup: a click landed on lens `index` (into the last vector
    // `setCodeLenses` was given). Only ever emitted for a `clickable` one —
    // running it, and what its answer means, is EditorTabs's job via
    // `LanguageService::runCodeLens`.
    void codeLensClicked(int index);

protected:
    void resizeEvent(QResizeEvent *event) override;
    // F1-15: the secondary carets are drawn over whatever the base class
    // painted. They do not blink: one timer driving two phases is how a
    // secondary caret ends up dark while the primary is lit, and a caret
    // you cannot see is worse than one that does not blink.
    void paintEvent(QPaintEvent *event) override;
    void mouseReleaseEvent(QMouseEvent *event) override;
    // Dead keys and CJK composition are not multi-caret operations — there
    // is no sensible meaning for a composition committed at 200 places — so
    // composing collapses to the primary caret first.
    void inputMethodEvent(QInputMethodEvent *event) override;
    // F1-8: routes plain-text paste through `EditorOps::pasteText` instead
    // of Qt's own insertion — the case every editor gets wrong is treating
    // paste as a run of keystrokes, which auto-close would then wrap.
    void insertFromMimeData(const QMimeData *source) override;
    // Right-click: Qt's own entries plus whatever the window appends, and
    // the caret moved under the pointer first so a gesture chosen from the
    // menu acts on what was clicked.
    void contextMenuEvent(QContextMenuEvent *event) override;
    void changeEvent(QEvent *event) override;
    // N7: Ctrl-hover feedback and Ctrl+Click activation, mirroring
    // TerminalWidget's clickable links (F4). Mouse tracking is on so a
    // move with no button held still arrives here.
    void mouseMoveEvent(QMouseEvent *event) override;
    // L3: QEvent::ToolTip is Qt's own dwell detection, and it arrives on the
    // viewport for a scroll area — so this, not event(), is where a hover
    // gesture is picked up.
    bool viewportEvent(QEvent *event) override;
    void mousePressEvent(QMouseEvent *event) override;
    void leaveEvent(QEvent *event) override;
    // L5: Ctrl+Space, the keys the popup owns while it is open, and the
    // per-keystroke completion request.
    void keyPressEvent(QKeyEvent *event) override;
    void focusOutEvent(QFocusEvent *event) override;

private slots:
    void updateLineNumberAreaWidth(int newBlockCount);
    void updateLineNumberArea(const QRect &rect, int dy);
    // Paints a full-width band behind the line holding the cursor, in
    // `currentLineBandColor()`.
    void highlightCurrentLine();

private:
    // The band colour: the explicit override when set, otherwise a tint of
    // the editor palette (set per-theme by MainWindow) so it follows the
    // theme by default.
    QColor currentLineBandColor() const;

    // The identifier-shaped word under `pos`, as a [start, end) document
    // position pair, or {-1, -1} when there is none. "Identifier-shaped"
    // is deliberately a spelling test (first character a letter or '_'),
    // not a resolution test: hovering must not cost an index query — see
    // ADR-0011.
    QPair<int, int> identifierAt(const QPoint &pos) const;
    void updateHoverSpan(const QPoint &pos, bool ctrlHeld);
    // Withdraws an outstanding hover request (pointer moved or left).
    void cancelHover();
    void clearHoverSpan();

    // The current line up to the caret — what `lsp_core::completion` reads
    // both the typed word and the trigger character out of.
    QString textBeforeCursor() const;
    // Accepts `entry`: announces the choice on completionChosen() and
    // dismisses the popup. The insertion itself arrives as a buffer edit.
    void insertCompletion(const CompletionEntry &entry);
    void hideCompletionPopup();

    bool foldStartingAt(int blockNumber, FoldRange *out) const;
    void toggleFold(int blockNumber);
    void setBlocksVisible(int fromBlockExclusive, int toBlockInclusive, bool visible);

    // F3-16: the marker painted at `blockNumber`, if any.
    bool changeMarkerAt(int blockNumber, ChangeMarker *out) const;

    // Show-whitespace-characters task: paints whichever of the whitespace
    // glyphs and the end-of-line marker are currently on, over the
    // currently visible blocks. Split out of paintEvent because it needs
    // its own two-pass shape (gather the visible blocks' text for one
    // batched classifier call, then paint from the answer) that would
    // otherwise crowd out the secondary-caret/inlay-hint painting above it.
    void paintWhitespace();
    void refreshTabStopDistance();
    // Positions the minimap strip against the current viewport, so both
    // resizeEvent and a live "Show minimap" toggle (which changes the
    // strip's width without a widget resize) place it the same way.
    void layoutMinimap();

    LineNumberArea *lineNumberArea_;
    Minimap *minimap_;
    // R1-7: set from RunService::canRunFile; widens the gutter by one icon
    // column and puts the Run triangle on the first line.
    bool runnable_ = false;
    // D2-5: breakpoints in this file, and the suspended line.
    QSet<int> breakpointLines_;
    int executionLine_ = -1;
    // F3-18: blame text keyed by block, and whether the gutter currently
    // widens to show it — same "empty means default, off by default"
    // arrangement inlay hints already use.
    QHash<int, QString> blameAnnotations_;
    bool blameEnabled_ = false;
    QVector<QPair<int, int>> matchSelections_;
    int currentMatch_ = -1;
    QString currentLineColor_;
    QVector<FoldRange> foldRanges_;
    // `foldRanges_` indexed by start block, so the gutter paint can ask
    // "does a fold start on this line?" without scanning. The gutter
    // repaints on every scroll step and asks once per painted line, so a
    // linear scan here made scrolling cost O(visible lines x fold ranges) —
    // which a multi-megabyte file has hundreds of thousands of.
    QHash<int, FoldRange> foldStarts_;
    QVector<FoldRange> collapsedRanges_;
    // F3-16: the gutter's change-marker strip, keyed by block like
    // foldStarts_ is — repainted on every scroll step.
    QHash<int, ChangeMarker> changeMarkers_;
    QVector<DiagnosticSpan> diagnosticSpans_;
    QVector<OccurrenceSpan> occurrenceSpans_;
    QVector<DiffLineBackground> diffBackgrounds_;
    QVector<DiffInlineSpan> diffSpans_;
    QVector<InlayHintSpan> inlayHints_;
    QVector<InlineValueSpan> inlineValues_;
    bool inlayHintsEnabled_ = false;
    QVector<CodeLensSpan> codeLenses_;
    // Screen rects the last paintEvent drew each of codeLenses_'s clickable
    // entries at, parallel to codeLenses_ by index — mousePressEvent hit-
    // tests against this rather than recomputing layout, the same
    // paint-then-hit-test split the gutter's change-marker strip uses.
    QHash<int, QRect> codeLensClickRects_;
    WhitespaceOptions whitespaceOptions_;
    WhitespaceClassifier whitespaceClassifier_;
    // Simple "recompute if the document revision or the visible block
    // range changed" cache (show-whitespace-characters task): classifying
    // one line is cheap, but it is still an FFI call, and paintEvent can
    // fire many times a second while scrolling or typing. -1 never
    // matches a real revision, so the first paint always computes once.
    int whitespaceCacheRevision_ = -1;
    int whitespaceCacheFirstBlock_ = -1;
    int whitespaceCacheLastBlock_ = -1;
    QVector<WhitespaceSpan> whitespaceCache_;
    // Tab width in columns, resolved for this tab's language
    // (`EditorOps::tabWidthForTab`); 4 is a reasonable pre-resolution
    // default, matching `settings_model::editing`'s own default.
    int tabWidthColumns_ = 4;
    // The Ctrl-hovered word, as [start, end) document positions, or
    // {-1, -1} for none. Pure view state, like the fold collapse state.
    QPair<int, int> hoverSpan_{-1, -1};
    // Whether a hover answer is still outstanding, so an idle mouse move
    // doesn't cross the FFI seam to cancel nothing.
    bool hoverPending_ = false;
    // L5: the popup. QCompleter is used in UnfilteredPopupCompletion mode —
    // its own prefix matching is deliberately bypassed, because filtering
    // and ordering belong to the server (`filterText`/`sortText`) and are
    // done in `lsp_core::completion` before the model is ever filled.
    QCompleter *completer_;
    QStandardItemModel *completionModel_;
    QVector<CompletionEntry> completionEntries_;
    // F1-15: every caret except this widget's own. Empty is the ordinary
    // single-caret editor, and every multi-caret code path below is guarded
    // on it, so nothing changes for a user who never presses Ctrl+D.
    QVector<SecondaryCaret> secondaryCarets_;
    // Where an Alt+Shift drag started, and whether one is in progress.
    int columnAnchor_ = -1;
    bool columnDragging_ = false;
};

// No Q_OBJECT: forwards paint events to CodeEditor, uses no signals/slots.
class LineNumberArea : public QWidget
{
public:
    explicit LineNumberArea(CodeEditor *editor)
      : QWidget(editor)
      , codeEditor_(editor)
    {
    }

    QSize sizeHint() const override { return QSize(codeEditor_->lineNumberAreaWidth(), 0); }

protected:
    void paintEvent(QPaintEvent *event) override { codeEditor_->lineNumberAreaPaintEvent(event); }
    void mousePressEvent(QMouseEvent *event) override
    {
        codeEditor_->lineNumberAreaMousePressEvent(event);
    }
    void contextMenuEvent(QContextMenuEvent *event) override
    {
        codeEditor_->lineNumberAreaContextMenuEvent(event);
    }

private:
    CodeEditor *codeEditor_;
};

} // namespace ui_shell
