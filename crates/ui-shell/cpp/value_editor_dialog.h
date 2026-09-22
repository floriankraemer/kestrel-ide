#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QDialog>

class QPlainTextEdit;
class QLabel;

namespace ui_shell {

// One cell's full-content editor (database-tools-plan F4.1), opened with
// Shift+Enter or a double-click on a grid cell: multi-line text, a JSON
// pretty-print toggle (`ResultProvider::prettyJson`, pure Rust), NULL/
// DEFAULT buttons, and load-from/save-to-file for a value too long to
// type comfortably inline.
//
// Humble view: every button calls straight through to `ResultProvider`;
// this dialog owns no rule about what a value *means*, only how it is
// typed in. A binary (`Bytes`) column has no dedicated hex mode here yet
// (ponytail: `HexViewer` is read-only-file-shaped today, not "edit a
// bound-parameter byte string" — add a binary mode if a real user asks
// for one; today's workaround is load-from-file with the bytes already
// hex-encoded, since `db_core::value::parse_text` accepts hex text for a
// binary column).
class ValueEditorDialog : public QDialog
{
    Q_OBJECT

public:
    ValueEditorDialog(ResultProvider *provider, quint64 resultId, quint64 row,
                      const QString &column, const QString &initialText, QWidget *parent);

private:
    void save();
    void setNull();
    void setDefault();
    void togglePrettyJson();
    void loadFromFile();
    void saveToFile();

    ResultProvider *provider_;
    quint64 resultId_;
    quint64 row_;
    QString column_;
    QPlainTextEdit *editor_;
    QLabel *statusLabel_;
    bool prettyShown_ = false;
    QString rawText_;
};

} // namespace ui_shell
