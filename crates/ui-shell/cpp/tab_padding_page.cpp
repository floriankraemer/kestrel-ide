#include "tab_padding_page.h"

#include "theme.h"

#include <QFormLayout>
#include <QLabel>
#include <QMessageBox>
#include <QSpinBox>
#include <QVBoxLayout>
#include <QWidget>

namespace ui_shell {

namespace {

// 0 and 100 are both real, allowed values (`app_config::tab_padding::
// MAX_TAB_PADDING`) — a spin box's own range is the first line of defence
// against "negative" (not representable anyway) and "absurdly high": it
// simply will not let the user type either.
constexpr int kMaxTabPadding = 100;

QSpinBox *addPaddingSpin(QFormLayout *form, const QString &label, quint32 value)
{
    auto *spin = new QSpinBox(form->parentWidget());
    spin->setRange(0, kMaxTabPadding);
    spin->setSuffix(QStringLiteral(" px"));
    spin->setValue(static_cast<int>(value));
    form->addRow(label, spin);
    return spin;
}

} // namespace

TabPaddingPage buildTabPaddingPage(QWidget *parent, AppSettings *appSettings)
{
    const FfiTabPadding current = appSettings->tabPadding();

    auto *page = new QWidget(parent);
    auto *layout = new QVBoxLayout(page);
    auto *form = new QFormLayout();
    layout->addLayout(form);

    QSpinBox *topSpin = addPaddingSpin(form, QObject::tr("Top:"), current.top);
    QSpinBox *bottomSpin = addPaddingSpin(form, QObject::tr("Bottom:"), current.bottom);
    QSpinBox *leftSpin = addPaddingSpin(form, QObject::tr("Left:"), current.left);
    QSpinBox *rightSpin = addPaddingSpin(form, QObject::tr("Right:"), current.right);

    auto *hint = new QLabel(
      QObject::tr("The right side also carries the tab's close button, which keeps its own "
                  "spacing regardless of this setting."),
      page);
    hint->setWordWrap(true);
    hint->setEnabled(false);
    layout->addWidget(hint);
    layout->addStretch(1);

    return TabPaddingPage{
      page,
      [appSettings, page, topSpin, bottomSpin, leftSpin, rightSpin]() {
          FfiTabPadding padding{};
          padding.top = static_cast<quint32>(topSpin->value());
          padding.bottom = static_cast<quint32>(bottomSpin->value());
          padding.left = static_cast<quint32>(leftSpin->value());
          padding.right = static_cast<quint32>(rightSpin->value());
          const FfiResult saved = appSettings->saveTabPadding(padding);
          if (saved.code != 0) {
              QMessageBox::critical(page->window(), QObject::tr("Tabs"), saved.message);
              return false;
          }
          applyTabPadding(padding);
          return true;
      },
    };
}

} // namespace ui_shell
