#include "database_exchange_actions.h"

#include <QCheckBox>
#include <QClipboard>
#include <QComboBox>
#include <QDialog>
#include <QDialogButtonBox>
#include <QDoubleSpinBox>
#include <QFileDialog>
#include <QFormLayout>
#include <QGuiApplication>
#include <QHBoxLayout>
#include <QImage>
#include <QLabel>
#include <QLineEdit>
#include <QListWidget>
#include <QMessageBox>
#include <QPixmap>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QSpinBox>
#include <QTableWidget>
#include <QVBoxLayout>

namespace ui_shell {

namespace {

// Every export format the dialog offers, in combo order — the display
// text is the only thing this file decides; which options each format
// actually uses is `ExchangeService`/`db_exchange::export`'s call.
struct FormatChoice
{
    QString label;
    FfiExportFormat format;
};

QVector<FormatChoice> exportFormatChoices()
{
    return {
      {QObject::tr("CSV"), FfiExportFormat::Csv},
      {QObject::tr("TSV"), FfiExportFormat::Tsv},
      {QObject::tr("JSON (array)"), FfiExportFormat::Json},
      {QObject::tr("JSON Lines"), FfiExportFormat::JsonLines},
      {QObject::tr("Markdown table"), FfiExportFormat::Markdown},
      {QObject::tr("HTML table"), FfiExportFormat::Html},
      {QObject::tr("SQL INSERT"), FfiExportFormat::SqlInsert},
      {QObject::tr("SQL UPDATE"), FfiExportFormat::SqlUpdate},
      {QObject::tr("Excel (XLSX)"), FfiExportFormat::Xlsx},
    };
}

// One export-options form, shared by the "Export data" and "Export rows"
// dialogs (the same knobs apply to both — a fresh streamed table and an
// already-fetched result grid) — populated into `layout`, read back by
// `readOptions`.
struct ExportOptionsForm
{
    QComboBox *format = nullptr;
    QCheckBox *header = nullptr;
    QLineEdit *nullText = nullptr;
    QCheckBox *quoteAll = nullptr;
    QLineEdit *delimiter = nullptr;
    QLineEdit *dateFormat = nullptr;
    QLineEdit *tableName = nullptr;
    QLineEdit *keyColumns = nullptr;
};

ExportOptionsForm buildExportOptionsForm(QFormLayout *layout)
{
    ExportOptionsForm form;
    form.format = new QComboBox();
    for (const FormatChoice &choice : exportFormatChoices()) {
        form.format->addItem(choice.label, QVariant::fromValue(static_cast<int>(choice.format)));
    }
    layout->addRow(QObject::tr("Format:"), form.format);

    form.header = new QCheckBox(QObject::tr("Header row"));
    form.header->setChecked(true);
    layout->addRow(QString(), form.header);

    form.nullText = new QLineEdit(QStringLiteral("NULL"));
    layout->addRow(QObject::tr("Null text:"), form.nullText);

    form.quoteAll = new QCheckBox(QObject::tr("Quote every field"));
    layout->addRow(QString(), form.quoteAll);

    form.delimiter = new QLineEdit(QStringLiteral(","));
    form.delimiter->setMaxLength(1);
    layout->addRow(QObject::tr("Delimiter (CSV/TSV):"), form.delimiter);

    form.dateFormat = new QLineEdit(QStringLiteral("%Y-%m-%d"));
    layout->addRow(QObject::tr("Date format:"), form.dateFormat);

    form.tableName = new QLineEdit(QStringLiteral("export"));
    layout->addRow(QObject::tr("Table name (SQL):"), form.tableName);

    form.keyColumns = new QLineEdit();
    form.keyColumns->setPlaceholderText(QObject::tr("id, other_key"));
    layout->addRow(QObject::tr("Key columns (SQL UPDATE):"), form.keyColumns);
    return form;
}

FfiExportFormat selectedFormat(const ExportOptionsForm &form)
{
    return static_cast<FfiExportFormat>(form.format->currentData().toInt());
}

FfiExportOptions readOptions(const ExportOptionsForm &form)
{
    FfiExportOptions options;
    options.header = form.header->isChecked();
    options.nullText = form.nullText->text();
    options.quoteAll = form.quoteAll->isChecked();
    options.delimiter = form.delimiter->text();
    options.dateFormat = form.dateFormat->text();
    options.tableName = form.tableName->text();
    options.keyColumns = form.keyColumns->text();
    return options;
}

// A modal progress view for one job id: a scrolling log fed by
// `jobProgress`, closes itself (reporting the outcome) on `jobFinished`.
// Shared by every job-backed action (export/import/copy/dump) so each
// dialog function only builds its own options form, not its own progress
// UI.
void runJobModally(QWidget *parent, ExchangeService *exchange, quint64 jobId, const QString &title)
{
    if (jobId == 0) {
        return; // `ExchangeService` already emitted `jobFinished(0, ...)`.
    }
    auto *dialog = new QDialog(parent);
    dialog->setWindowTitle(title);
    dialog->setAttribute(Qt::WA_DeleteOnClose);
    auto *layout = new QVBoxLayout(dialog);
    auto *log = new QPlainTextEdit(dialog);
    log->setReadOnly(true);
    layout->addWidget(log);
    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Close, dialog);
    buttons->button(QDialogButtonBox::Close)->setEnabled(false);
    layout->addWidget(buttons);
    QObject::connect(buttons, &QDialogButtonBox::rejected, dialog, &QDialog::reject);

    QObject::connect(exchange, &ExchangeService::jobProgress, dialog,
                     [log, jobId](quint64 id, quint64 done, quint64 total, const QString &message) {
                         if (id != jobId) {
                             return;
                         }
                         if (!message.isEmpty()) {
                             log->appendPlainText(message);
                         } else {
                             log->appendPlainText(QStringLiteral("%1/%2").arg(done).arg(total));
                         }
                     });
    QObject::connect(
      exchange, &ExchangeService::jobFinished, dialog,
      [log, buttons, jobId](quint64 id, bool ok, const QString &message, const QString &outputPath) {
          if (id != jobId) {
              return;
          }
          log->appendPlainText(ok ? QObject::tr("Done: %1").arg(message)
                                  : QObject::tr("Failed: %1").arg(message));
          if (!outputPath.isEmpty()) {
              log->appendPlainText(QObject::tr("Output: %1").arg(outputPath));
          }
          buttons->button(QDialogButtonBox::Close)->setEnabled(true);
      });
    dialog->resize(520, 320);
    dialog->show();
}

} // namespace

void showExportDataDialog(QWidget *parent, ExchangeService *exchange, const QString &sourceId,
                          const QString &objectPath)
{
    QDialog dialog(parent);
    dialog.setWindowTitle(QObject::tr("Export Data — %1").arg(objectPath));
    auto *outer = new QVBoxLayout(&dialog);
    auto *form = new QFormLayout();
    outer->addLayout(form);
    ExportOptionsForm optionsForm = buildExportOptionsForm(form);

    auto *destinationEdit = new QLineEdit();
    auto *browseButton = new QPushButton(QObject::tr("Browse…"));
    auto *destinationRow = new QHBoxLayout();
    destinationRow->addWidget(destinationEdit, 1);
    destinationRow->addWidget(browseButton);
    form->addRow(QObject::tr("Destination file:"), destinationRow);
    QObject::connect(browseButton, &QPushButton::clicked, &dialog, [&dialog, destinationEdit]() {
        const QString path = QFileDialog::getSaveFileName(&dialog, QObject::tr("Export Data"));
        if (!path.isEmpty()) {
            destinationEdit->setText(path);
        }
    });

    auto *previewButton = new QPushButton(QObject::tr("Preview first rows"));
    auto *previewText = new QPlainTextEdit();
    previewText->setReadOnly(true);
    previewText->setMaximumHeight(140);
    outer->addWidget(previewButton);
    outer->addWidget(previewText);
    QObject::connect(previewButton, &QPushButton::clicked, &dialog,
                     [exchange, sourceId, objectPath, &optionsForm, previewText]() {
                         previewText->setPlainText(exchange->exportPreviewText(
                           sourceId, objectPath, selectedFormat(optionsForm), readOptions(optionsForm), 20));
                     });

    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, &dialog);
    outer->addWidget(buttons);
    QObject::connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
    QObject::connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);

    if (dialog.exec() != QDialog::Accepted) {
        return;
    }
    if (destinationEdit->text().isEmpty()) {
        QMessageBox::warning(parent, QObject::tr("Export Data"), QObject::tr("Choose a destination file."));
        return;
    }
    const quint64 jobId = exchange->exportTable(sourceId, objectPath, selectedFormat(optionsForm),
                                                readOptions(optionsForm), destinationEdit->text());
    runJobModally(parent, exchange, jobId, QObject::tr("Exporting %1").arg(objectPath));
}

void showExportRowsDialog(QWidget *parent, ExchangeService *exchange, const QStringList &columns,
                          const ::rust::Vec<FfiDbRow> &rows)
{
    QDialog dialog(parent);
    dialog.setWindowTitle(QObject::tr("Export / Copy As"));
    auto *outer = new QVBoxLayout(&dialog);
    auto *form = new QFormLayout();
    outer->addLayout(form);
    ExportOptionsForm optionsForm = buildExportOptionsForm(form);

    auto *toClipboard = new QCheckBox(QObject::tr("Copy to clipboard instead of a file"));
    toClipboard->setChecked(true);
    outer->addWidget(toClipboard);
    auto *destinationEdit = new QLineEdit();
    auto *browseButton = new QPushButton(QObject::tr("Browse…"));
    auto *destinationRow = new QHBoxLayout();
    destinationRow->addWidget(destinationEdit, 1);
    destinationRow->addWidget(browseButton);
    form->addRow(QObject::tr("Destination file:"), destinationRow);
    QObject::connect(browseButton, &QPushButton::clicked, &dialog, [&dialog, destinationEdit]() {
        const QString path = QFileDialog::getSaveFileName(&dialog, QObject::tr("Export"));
        if (!path.isEmpty()) {
            destinationEdit->setText(path);
        }
    });

    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, &dialog);
    outer->addWidget(buttons);
    QObject::connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
    QObject::connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);

    if (dialog.exec() != QDialog::Accepted) {
        return;
    }
    const FfiExportFormat format = selectedFormat(optionsForm);
    const FfiExportOptions options = readOptions(optionsForm);
    if (toClipboard->isChecked()) {
        const QString text = exchange->exportRowsToText(columns, rows, format, options);
        QGuiApplication::clipboard()->setText(text);
        return;
    }
    if (destinationEdit->text().isEmpty()) {
        QMessageBox::warning(parent, QObject::tr("Export"), QObject::tr("Choose a destination file."));
        return;
    }
    const FfiResult result = exchange->exportRowsToFile(columns, rows, format, options, destinationEdit->text());
    if (result.code != 0) {
        QMessageBox::warning(parent, QObject::tr("Export"), QString(result.message));
    }
}

void showImportDataDialog(QWidget *parent, ExchangeService *exchange, const QString &sourceId,
                          const QString &targetTable)
{
    QDialog dialog(parent);
    dialog.setWindowTitle(QObject::tr("Import Data — %1").arg(targetTable));
    auto *outer = new QVBoxLayout(&dialog);

    auto *fileRow = new QHBoxLayout();
    auto *fileEdit = new QLineEdit();
    auto *browseButton = new QPushButton(QObject::tr("Browse…"));
    fileRow->addWidget(fileEdit, 1);
    fileRow->addWidget(browseButton);
    outer->addLayout(fileRow);

    auto *mappingTable = new QTableWidget(0, 4, &dialog);
    mappingTable->setHorizontalHeaderLabels(
      {QObject::tr("Source column"), QObject::tr("Target column"), QObject::tr("Type"), QObject::tr("Skip")});
    outer->addWidget(mappingTable, 1);

    auto *targetTableEdit = new QLineEdit(targetTable);
    auto *createTable = new QCheckBox(QObject::tr("Create table if it does not exist"));
    createTable->setChecked(true);
    auto *batchSize = new QSpinBox();
    batchSize->setRange(1, 100000);
    batchSize->setValue(500);
    auto *optionsForm = new QFormLayout();
    optionsForm->addRow(QObject::tr("Target table:"), targetTableEdit);
    optionsForm->addRow(QString(), createTable);
    optionsForm->addRow(QObject::tr("Batch size:"), batchSize);
    outer->addLayout(optionsForm);

    auto rebuildMapping = [mappingTable](const FfiImportPreview &preview) {
        mappingTable->setRowCount(0);
        for (qsizetype i = 0; i < preview.columns.size(); ++i) {
            const int row = mappingTable->rowCount();
            mappingTable->insertRow(row);
            mappingTable->setItem(row, 0, new QTableWidgetItem(preview.columns.at(i)));
            mappingTable->setItem(row, 1, new QTableWidgetItem(preview.columns.at(i)));
            mappingTable->setItem(row, 2, new QTableWidgetItem(preview.detectedTypes.at(i)));
            auto *skip = new QTableWidgetItem();
            skip->setFlags(skip->flags() | Qt::ItemIsUserCheckable);
            skip->setCheckState(Qt::Unchecked);
            mappingTable->setItem(row, 3, skip);
        }
    };
    QObject::connect(browseButton, &QPushButton::clicked, &dialog, [&dialog, fileEdit, exchange, rebuildMapping]() {
        const QString path = QFileDialog::getOpenFileName(&dialog, QObject::tr("Import Data"), QString(),
                                                           QObject::tr("CSV/XLSX (*.csv *.xlsx)"));
        if (path.isEmpty()) {
            return;
        }
        fileEdit->setText(path);
        FfiImportOptions options;
        options.header = true;
        options.nullText = QString();
        options.delimiter = QStringLiteral(",");
        options.dateFormat = QStringLiteral("%Y-%m-%d");
        rebuildMapping(exchange->importPreview(path, options));
    });

    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, &dialog);
    outer->addWidget(buttons);
    QObject::connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
    QObject::connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    dialog.resize(560, 420);

    if (dialog.exec() != QDialog::Accepted) {
        return;
    }
    if (fileEdit->text().isEmpty()) {
        QMessageBox::warning(parent, QObject::tr("Import Data"), QObject::tr("Choose a source file."));
        return;
    }
    ::rust::Vec<FfiImportColumnMapping> mapping;
    for (int row = 0; row < mappingTable->rowCount(); ++row) {
        FfiImportColumnMapping column;
        column.sourceCol = mappingTable->item(row, 0)->text();
        column.targetCol = mappingTable->item(row, 1)->text();
        column.typeCoercion = mappingTable->item(row, 2)->text();
        column.skip = mappingTable->item(row, 3)->checkState() == Qt::Checked;
        mapping.push_back(column);
    }
    FfiImportOptions options;
    options.header = true;
    options.nullText = QString();
    options.delimiter = QStringLiteral(",");
    options.dateFormat = QStringLiteral("%Y-%m-%d");
    options.createTable = createTable->isChecked();
    options.batchSize = static_cast<quint32>(batchSize->value());
    const quint64 jobId =
      exchange->importRun(sourceId, fileEdit->text(), targetTableEdit->text(), mapping, options);
    runJobModally(parent, exchange, jobId, QObject::tr("Importing into %1").arg(targetTableEdit->text()));
}

void showCopyTableDialog(QWidget *parent, ExchangeService *exchange, const QString &srcSourceId,
                         const QString &srcTable, const ::rust::Vec<FfiDbSourceRow> &sources)
{
    QDialog dialog(parent);
    dialog.setWindowTitle(QObject::tr("Copy Table — %1").arg(srcTable));
    auto *form = new QFormLayout(&dialog);

    auto *destSource = new QComboBox();
    for (const FfiDbSourceRow &source : sources) {
        destSource->addItem(QString(source.name), QString(source.id));
    }
    form->addRow(QObject::tr("Destination source:"), destSource);

    auto *destTable = new QLineEdit(srcTable);
    form->addRow(QObject::tr("Destination table:"), destTable);

    auto *createIfMissing = new QCheckBox(QObject::tr("Create table if it does not exist"));
    createIfMissing->setChecked(true);
    form->addRow(QString(), createIfMissing);

    auto *batchSize = new QSpinBox();
    batchSize->setRange(1, 100000);
    batchSize->setValue(500);
    form->addRow(QObject::tr("Batch size:"), batchSize);

    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, &dialog);
    form->addRow(buttons);
    QObject::connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
    QObject::connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);

    if (dialog.exec() != QDialog::Accepted) {
        return;
    }
    const quint64 jobId =
      exchange->copyTable(srcSourceId, srcTable, destSource->currentData().toString(), destTable->text(),
                          createIfMissing->isChecked(), static_cast<quint32>(batchSize->value()));
    runJobModally(parent, exchange, jobId, QObject::tr("Copying %1").arg(srcTable));
}

void showDumpDialog(QWidget *parent, ExchangeService *exchange, const QString &sourceId)
{
    QDialog dialog(parent);
    dialog.setWindowTitle(QObject::tr("Dump — %1").arg(sourceId));
    auto *outer = new QVBoxLayout(&dialog);

    const FfiDumpToolStatus status = exchange->dumpToolStatus(sourceId);
    auto *statusLabel = new QLabel(status.available
                                     ? QObject::tr("%1 found on PATH.").arg(QString(status.program))
                                     : QObject::tr("%1 not found. %2")
                                         .arg(QString(status.program), QString(status.installHint)));
    statusLabel->setWordWrap(true);
    outer->addWidget(statusLabel);

    auto *form = new QFormLayout();
    outer->addLayout(form);
    auto *schemaOnly = new QCheckBox(QObject::tr("Schema only"));
    auto *dataOnly = new QCheckBox(QObject::tr("Data only"));
    form->addRow(QString(), schemaOnly);
    form->addRow(QString(), dataOnly);
    auto *tables = new QLineEdit();
    tables->setPlaceholderText(QObject::tr("Every table, or a comma-separated subset"));
    form->addRow(QObject::tr("Tables:"), tables);
    auto *outputEdit = new QLineEdit();
    auto *browseButton = new QPushButton(QObject::tr("Browse…"));
    auto *outputRow = new QHBoxLayout();
    outputRow->addWidget(outputEdit, 1);
    outputRow->addWidget(browseButton);
    form->addRow(QObject::tr("Output file:"), outputRow);
    QObject::connect(browseButton, &QPushButton::clicked, &dialog, [&dialog, outputEdit]() {
        const QString path = QFileDialog::getSaveFileName(&dialog, QObject::tr("Dump Output"));
        if (!path.isEmpty()) {
            outputEdit->setText(path);
        }
    });

    auto *argvPreview = new QPlainTextEdit();
    argvPreview->setReadOnly(true);
    argvPreview->setMaximumHeight(60);
    outer->addWidget(argvPreview);
    auto refreshPreview = [exchange, sourceId, schemaOnly, dataOnly, tables, outputEdit, argvPreview]() {
        FfiDumpOptions options;
        options.schemaOnly = schemaOnly->isChecked();
        options.dataOnly = dataOnly->isChecked();
        options.tables = tables->text();
        options.outputFile = outputEdit->text();
        argvPreview->setPlainText(exchange->dumpArgvPreview(sourceId, options));
    };
    QObject::connect(schemaOnly, &QCheckBox::toggled, &dialog, refreshPreview);
    QObject::connect(dataOnly, &QCheckBox::toggled, &dialog, refreshPreview);
    QObject::connect(tables, &QLineEdit::textChanged, &dialog, refreshPreview);
    QObject::connect(outputEdit, &QLineEdit::textChanged, &dialog, refreshPreview);
    refreshPreview();

    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, &dialog);
    outer->addWidget(buttons);
    QObject::connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
    QObject::connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);

    if (dialog.exec() != QDialog::Accepted) {
        return;
    }
    if (outputEdit->text().isEmpty()) {
        QMessageBox::warning(parent, QObject::tr("Dump"), QObject::tr("Choose an output file."));
        return;
    }
    FfiDumpOptions options;
    options.schemaOnly = schemaOnly->isChecked();
    options.dataOnly = dataOnly->isChecked();
    options.tables = tables->text();
    options.outputFile = outputEdit->text();
    const quint64 jobId = exchange->dump(sourceId, options);
    runJobModally(parent, exchange, jobId, QObject::tr("Dumping %1").arg(sourceId));
}

void showRestoreDialog(QWidget *parent, ExchangeService *exchange, const QString &sourceId)
{
    QDialog dialog(parent);
    dialog.setWindowTitle(QObject::tr("Restore — %1").arg(sourceId));
    auto *outer = new QVBoxLayout(&dialog);

    auto *warning = new QLabel(
      QObject::tr("This writes into '%1'. Existing data may be overwritten or duplicated.")
        .arg(sourceId));
    warning->setWordWrap(true);
    outer->addWidget(warning);

    auto *form = new QFormLayout();
    outer->addLayout(form);
    auto *inputEdit = new QLineEdit();
    auto *browseButton = new QPushButton(QObject::tr("Browse…"));
    auto *inputRow = new QHBoxLayout();
    inputRow->addWidget(inputEdit, 1);
    inputRow->addWidget(browseButton);
    form->addRow(QObject::tr("Dump file:"), inputRow);
    QObject::connect(browseButton, &QPushButton::clicked, &dialog, [&dialog, inputEdit]() {
        const QString path = QFileDialog::getOpenFileName(&dialog, QObject::tr("Restore From"));
        if (!path.isEmpty()) {
            inputEdit->setText(path);
        }
    });

    auto *argvPreview = new QPlainTextEdit();
    argvPreview->setReadOnly(true);
    argvPreview->setMaximumHeight(60);
    outer->addWidget(argvPreview);
    auto refreshPreview = [exchange, sourceId, inputEdit, argvPreview]() {
        argvPreview->setPlainText(exchange->restoreArgvPreview(sourceId, inputEdit->text()));
    };
    QObject::connect(inputEdit, &QLineEdit::textChanged, &dialog, refreshPreview);
    refreshPreview();

    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, &dialog);
    buttons->button(QDialogButtonBox::Ok)->setText(QObject::tr("Restore"));
    outer->addWidget(buttons);
    QObject::connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
    QObject::connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);

    if (dialog.exec() != QDialog::Accepted) {
        return;
    }
    if (inputEdit->text().isEmpty()) {
        QMessageBox::warning(parent, QObject::tr("Restore"), QObject::tr("Choose a dump file."));
        return;
    }
    if (QMessageBox::question(parent, QObject::tr("Restore"),
                              QObject::tr("Restore '%1' from '%2'? This may overwrite existing data.")
                                .arg(sourceId, inputEdit->text()))
        != QMessageBox::Yes) {
        return;
    }
    const quint64 jobId = exchange->restore(sourceId, inputEdit->text());
    runJobModally(parent, exchange, jobId, QObject::tr("Restoring %1").arg(sourceId));
}

void showErDiagramDialog(QWidget *parent, ExchangeService *exchange, DocumentManager *documentManager,
                         const QString &sourceId, const QString &tableScope)
{
    auto *dialog = new QDialog(parent);
    dialog->setAttribute(Qt::WA_DeleteOnClose);
    dialog->setWindowTitle(QObject::tr("ER Diagram — %1").arg(tableScope.isEmpty() ? sourceId : tableScope));
    auto *layout = new QVBoxLayout(dialog);
    auto *imageLabel = new QLabel(dialog);
    imageLabel->setAlignment(Qt::AlignCenter);
    imageLabel->setMinimumSize(480, 320);
    layout->addWidget(imageLabel, 1);

    const FfiPreviewImage image = exchange->erDiagramImage(sourceId, tableScope, 800);
    if (image.width > 0 && image.height > 0) {
        const QImage decoded(reinterpret_cast<const uchar *>(image.pixels.constData()),
                             static_cast<int>(image.width), static_cast<int>(image.height),
                             QImage::Format_RGBA8888_Premultiplied);
        imageLabel->setPixmap(QPixmap::fromImage(decoded.copy()));
    } else {
        imageLabel->setText(QObject::tr("No tables to diagram."));
    }

    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Close, dialog);
    auto *viewAsText = new QPushButton(QObject::tr("View as Mermaid Text"), dialog);
    buttons->addButton(viewAsText, QDialogButtonBox::ActionRole);
    layout->addWidget(buttons);
    QObject::connect(buttons, &QDialogButtonBox::rejected, dialog, &QDialog::reject);
    QObject::connect(viewAsText, &QPushButton::clicked, dialog,
                     [exchange, documentManager, sourceId, tableScope]() {
                         const QString mermaid = exchange->erDiagramMermaid(sourceId, tableScope);
                         documentManager->openVirtualDocument(QStringLiteral("mermaid"),
                                                              QStringLiteral("%1/erd.mmd").arg(sourceId),
                                                              mermaid);
                     });
    dialog->show();
}

void showSchemaCompareDialog(QWidget *parent, ExchangeService *exchange,
                             DocumentManager *documentManager, const QString &leftSourceId,
                             const ::rust::Vec<FfiDbSourceRow> &sources)
{
    QDialog dialog(parent);
    dialog.setWindowTitle(QObject::tr("Compare Structure — %1").arg(leftSourceId));
    auto *outer = new QVBoxLayout(&dialog);
    auto *rightCombo = new QComboBox();
    for (const FfiDbSourceRow &source : sources) {
        if (QString(source.id) == leftSourceId) {
            continue;
        }
        rightCombo->addItem(QString(source.name), QString(source.id));
    }
    outer->addWidget(rightCombo);
    auto *compareButton = new QPushButton(QObject::tr("Compare"));
    outer->addWidget(compareButton);
    auto *resultList = new QListWidget();
    outer->addWidget(resultList, 1);
    auto *diffButton = new QPushButton(QObject::tr("Show Diff"));
    auto *migrationButton = new QPushButton(QObject::tr("Migration Script"));
    auto *buttonRow = new QHBoxLayout();
    buttonRow->addWidget(diffButton);
    buttonRow->addWidget(migrationButton);
    outer->addLayout(buttonRow);

    QObject::connect(compareButton, &QPushButton::clicked, &dialog,
                     [exchange, leftSourceId, rightCombo, resultList]() {
                         resultList->clear();
                         const QString rightSourceId = rightCombo->currentData().toString();
                         for (const FfiCompareRow &row : exchange->schemaCompare(leftSourceId, rightSourceId)) {
                             auto *item = new QListWidgetItem(
                               QStringLiteral("[%1] %2").arg(QString(row.kind), QString(row.name)));
                             item->setData(Qt::UserRole, QString(row.table));
                             item->setData(Qt::UserRole + 1, QString(row.name));
                             resultList->addItem(item);
                         }
                     });
    QObject::connect(diffButton, &QPushButton::clicked, &dialog,
                     [exchange, documentManager, leftSourceId, rightCombo, resultList]() {
                         QListWidgetItem *item = resultList->currentItem();
                         if (item == nullptr) {
                             return;
                         }
                         const QString rightSourceId = rightCombo->currentData().toString();
                         const FfiTextDiff diff =
                           exchange->ddlDiffTexts(leftSourceId, rightSourceId, item->data(Qt::UserRole).toString(),
                                                  item->data(Qt::UserRole + 1).toString());
                         documentManager->openDiffTab(QStringLiteral("%1.sql").arg(QString(diff.label)),
                                                      leftSourceId, rightSourceId, diff.left, diff.right);
                     });
    QObject::connect(migrationButton, &QPushButton::clicked, &dialog,
                     [exchange, documentManager, leftSourceId, rightCombo]() {
                         const QString rightSourceId = rightCombo->currentData().toString();
                         const QString script = exchange->migrationScript(leftSourceId, rightSourceId);
                         documentManager->openVirtualDocument(
                           QStringLiteral("db-migration"),
                           QStringLiteral("%1-to-%2.sql").arg(leftSourceId, rightSourceId), script);
                     });

    auto *close = new QDialogButtonBox(QDialogButtonBox::Close, &dialog);
    outer->addWidget(close);
    QObject::connect(close, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    dialog.resize(520, 420);
    dialog.exec();
}

void showDataCompareDialog(QWidget *parent, ExchangeService *exchange,
                           DocumentManager *documentManager, const QString &leftSourceId,
                           const QString &leftTable, const ::rust::Vec<FfiDbSourceRow> &sources)
{
    QDialog dialog(parent);
    dialog.setWindowTitle(QObject::tr("Compare Data — %1").arg(leftTable));
    auto *form = new QFormLayout(&dialog);

    auto *rightCombo = new QComboBox();
    for (const FfiDbSourceRow &source : sources) {
        rightCombo->addItem(QString(source.name), QString(source.id));
    }
    form->addRow(QObject::tr("Right source:"), rightCombo);
    auto *rightTable = new QLineEdit(leftTable);
    form->addRow(QObject::tr("Right table:"), rightTable);
    auto *keyColumns = new QLineEdit();
    keyColumns->setPlaceholderText(QObject::tr("id, other_key"));
    form->addRow(QObject::tr("Key columns:"), keyColumns);
    auto *tolerance = new QDoubleSpinBox();
    tolerance->setDecimals(6);
    tolerance->setRange(0.0, 1000.0);
    form->addRow(QObject::tr("Float tolerance:"), tolerance);

    auto *summaryLabel = new QLabel();
    form->addRow(summaryLabel);
    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Cancel, &dialog);
    auto *compareButton = new QPushButton(QObject::tr("Compare"));
    buttons->addButton(compareButton, QDialogButtonBox::ActionRole);
    auto *diffButton = new QPushButton(QObject::tr("Show Row Diff"));
    buttons->addButton(diffButton, QDialogButtonBox::ActionRole);
    form->addRow(buttons);
    QObject::connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);

    FfiDataCompareResult lastResult;
    QObject::connect(
      compareButton, &QPushButton::clicked, &dialog,
      [exchange, leftSourceId, leftTable, rightCombo, rightTable, keyColumns, tolerance, summaryLabel,
       &lastResult]() {
          lastResult = exchange->dataCompare(leftSourceId, leftTable, rightCombo->currentData().toString(),
                                             rightTable->text(), keyColumns->text(), tolerance->value());
          summaryLabel->setText(QObject::tr("Only left: %1  Only right: %2  Changed: %3  Equal: %4")
                                  .arg(lastResult.onlyLeft)
                                  .arg(lastResult.onlyRight)
                                  .arg(lastResult.changed)
                                  .arg(lastResult.equal));
      });
    QObject::connect(diffButton, &QPushButton::clicked, &dialog,
                     [documentManager, leftTable, rightTable, &lastResult]() {
                         documentManager->openDiffTab(QStringLiteral("%1.tsv").arg(leftTable), leftTable,
                                                      rightTable->text(), lastResult.leftText,
                                                      lastResult.rightText);
                     });

    dialog.exec();
}

} // namespace ui_shell
