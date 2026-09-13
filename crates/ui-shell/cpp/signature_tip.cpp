#include "signature_tip.h"

#include "editor_popup.h"

namespace ui_shell {

void showSignatureTip(QWidget *, const QPoint &globalPos, const FfiSignatureHelp &help)
{
    if (!help.has_signature) {
        hideSignatureTip();
        return;
    }
    const QString label = QString(help.label);
    QString html;
    if (help.has_active_parameter && help.parameter_start < help.parameter_end
        && help.parameter_end <= static_cast<quint32>(label.size())) {
        html = label.left(static_cast<int>(help.parameter_start)).toHtmlEscaped()
          + QStringLiteral("<b>")
          + label
              .mid(static_cast<int>(help.parameter_start),
                   static_cast<int>(help.parameter_end - help.parameter_start))
              .toHtmlEscaped()
          + QStringLiteral("</b>")
          + label.mid(static_cast<int>(help.parameter_end)).toHtmlEscaped();
    } else {
        html = label.toHtmlEscaped();
    }
    if (help.signature_count > 1) {
        html = QStringLiteral("(%1/%2)&nbsp;&nbsp;")
                 .arg(help.signature_index + 1)
                 .arg(help.signature_count)
          + html;
    }
    const QString documentation = QString(help.documentation);
    if (!documentation.isEmpty()) {
        html += QStringLiteral("<br/><span style=\"color:gray;\">")
          + documentation.toHtmlEscaped() + QStringLiteral("</span>");
    }
    // R3: the active parameter's own documentation, distinct from the
    // signature's — the reason `<b>...</b>` matters is knowing which
    // parameter this describes.
    const QString parameterDocumentation = QString(help.parameter_documentation);
    if (!parameterDocumentation.isEmpty()) {
        html += QStringLiteral("<br/><span style=\"color:gray;\">")
          + parameterDocumentation.toHtmlEscaped() + QStringLiteral("</span>");
    }
    showEditorPopup(globalPos, html);
}

void hideSignatureTip()
{
    hideEditorPopup();
}

} // namespace ui_shell
