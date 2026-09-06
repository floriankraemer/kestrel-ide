#include "help_menu.h"

#include "e2e_mark.h"
#include "keymap_page.h"
#include "ui_tokens.h"

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QAction>
#include <QDialog>
#include <QDialogButtonBox>
#include <QFont>
#include <QGridLayout>
#include <QHBoxLayout>
#include <QIcon>
#include <QLabel>
#include <QMainWindow>
#include <QMenu>
#include <QMenuBar>
#include <QPixmap>
#include <QPushButton>
#include <QVBoxLayout>

namespace ui_shell {

namespace {

// One "Label: value" pair in the metadata grid. Monospaced values are not
// worth a second font here — the hash is short and the date is fixed-width.
void addRow(QGridLayout *grid, int row, const QString &label, const QString &value)
{
    auto *name = new QLabel(label);
    name->setEnabled(false); // reads as the theme's muted foreground
    grid->addWidget(name, row, 0);

    auto *content = new QLabel(value);
    content->setTextInteractionFlags(Qt::TextSelectableByMouse);
    grid->addWidget(content, row, 1);
}

void showAboutDialog(QWidget *parent, AppInfo *info)
{
    QDialog dialog(parent);
    dialog.setWindowTitle(QObject::tr("About %1").arg(info->appName()));
    dialog.setModal(true);
    // Enough width that the repository link is not flush against the frame;
    // the layout still grows past this for a longer translated label.
    dialog.setMinimumWidth(380);

    auto *layout = new QVBoxLayout(&dialog);
    layout->setContentsMargins(tokens::kSp4, tokens::kSp4, tokens::kSp4, tokens::kSp4);
    layout->setSpacing(tokens::kSp3);

    auto *icon = new QLabel;
    icon->setPixmap(QIcon(QStringLiteral(":/ui/icons/app_icon.png")).pixmap(64, 64));
    icon->setAlignment(Qt::AlignCenter);
    layout->addWidget(icon);

    auto *name = new QLabel(info->appName());
    QFont nameFont = name->font();
    nameFont.setPointSizeF(nameFont.pointSizeF() * 1.6);
    nameFont.setBold(true);
    name->setFont(nameFont);
    name->setAlignment(Qt::AlignCenter);
    layout->addWidget(name);

    auto *grid = new QGridLayout;
    grid->setHorizontalSpacing(tokens::kSp3);
    grid->setVerticalSpacing(tokens::kSp1);
    addRow(grid, 0, QObject::tr("Version"), info->appVersion());
    addRow(grid, 1, QObject::tr("Commit"), info->gitHash());
    addRow(grid, 2, QObject::tr("Commit date"), info->gitDate());
    // Centred as a block, not stretched to the dialog's width: the icon, the
    // name and the link are all centred, and a grid pinned to the left edge
    // reads as a misalignment rather than as a deliberate table.
    auto *gridRow = new QHBoxLayout;
    gridRow->addStretch();
    gridRow->addLayout(grid);
    gridRow->addStretch();
    layout->addLayout(gridRow);

    // The one place in the app that hands a URL to the desktop browser.
    // `setOpenExternalLinks` rather than a QDesktopServices call of our own:
    // the label already knows the href, and a second copy of it in a lambda
    // is a second thing to keep in sync.
    auto *link = new QLabel(
      QStringLiteral("<a href=\"%1\">%1</a>").arg(info->projectUrl().toHtmlEscaped()));
    link->setOpenExternalLinks(true);
    link->setTextInteractionFlags(Qt::TextBrowserInteraction);
    link->setAlignment(Qt::AlignCenter);
    layout->addWidget(link);

    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Close);
    QObject::connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    layout->addWidget(buttons);

    e2eMark(QStringLiteral("{\"ev\":\"dialog_shown\",\"name\":\"about_dialog\",\"version\":%1,"
                            "\"hash\":%2}")
              .arg(e2eJson(info->appVersion()), e2eJson(info->gitHash())));
    dialog.exec();
    e2eMark("{\"ev\":\"dialog_closed\",\"name\":\"about_dialog\"}");
}

} // namespace

void buildHelpMenu(QMainWindow *window, AppSettings *appSettings,
                    QHash<QString, QAction *> &actions)
{
    QMenu *helpMenu = window->menuBar()->addMenu(QObject::tr("&Help"));
    // Same reasoning as `vcs_menu.cpp`: a menu-bar entry never goes through
    // `exec()`, so `aboutToShow`/`aboutToHide` are an E2E flow's only signal
    // that the popup is a live toplevel it can type into.
    QObject::connect(helpMenu, &QMenu::aboutToShow, helpMenu,
                      []() { e2eMark("{\"ev\":\"dialog_shown\",\"name\":\"help_menu\"}"); });
    QObject::connect(helpMenu, &QMenu::aboutToHide, helpMenu,
                      []() { e2eMark("{\"ev\":\"dialog_closed\",\"name\":\"help_menu\"}"); });

    // Stateless constants, so one for the process and never freed — the same
    // shape (and the same reasoning) as `sharedIconProvider()` in
    // icon_cache.cpp. Parentless on purpose: it has nothing to be destroyed
    // with, and the menu outlives the window anyway.
    auto *info = new AppInfo();

    QAction *aboutAction =
      registerAction(helpMenu, QStringLiteral("help.about"),
                      QObject::tr("About %1").arg(info->appName()), appSettings, actions);
    // AboutRole keeps macOS from burying this in the Help menu it would
    // otherwise merge; harmless everywhere else.
    aboutAction->setMenuRole(QAction::AboutRole);
    QObject::connect(aboutAction, &QAction::triggered, window,
                      [window, info]() { showAboutDialog(window, info); });
}

} // namespace ui_shell
