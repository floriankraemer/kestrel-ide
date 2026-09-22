#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QDialog>

class QPlainTextEdit;
class QLabel;

namespace ui_shell {

// One cell's full-content editor (database-tools-plan F4.1), opened with
// Shift+Enter or a double-click on a grid cell: multi-line text, a JSON
// pretty-print toggle (`ResultProvider::prettyJson`, pure Rust), NULL/
// DEFAULT buttons, load-from/save-to-file for a value too long to type
// comfortably inline, and a hex-edit mode for a binary (`Bytes`) column
// (F4c): `ResultProvider::isBinaryColumn` decides whether this opens in
// that mode, `ResultProvider::validateHex` gives live feedback as the
// user types, and `setCell`'s own `db_core::value::parse_text` still does
// the actual (re-)validation and coercion on Save — this dialog never
// parses hex itself.
//
// Humble view: every button/keystroke calls straight through to
// `ResultProvider`; this dialog owns no rule about what a value *means*,
// only how it is typed in.
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
    void validateHexAsTyped();

    ResultProvider *provider_;
    quint64 resultId_;
    quint64 row_;
    QString column_;
    QPlainTextEdit *editor_;
    QLabel *statusLabel_;
    QLabel *hexHintLabel_ = nullptr;
    bool prettyShown_ = false;
    bool isBinary_ = false;
    QString rawText_;
};

} // namespace ui_shell
