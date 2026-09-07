#include "symbol_icon.h"

#include "theme.h"

#include <QColor>

namespace ui_shell {

QIcon symbolKindIcon(FfiSymbolKind kind)
{
    // Each icon is built once, the first time its kind is requested, and
    // kept forever — a function-local static per case is legal even
    // though the switch jumps over the other cases' declarations (only
    // automatic-storage-duration variables forbid that).
    switch (kind) {
    case FfiSymbolKind::Class: {
        static const QIcon icon = maskIcon(":/ui/icons/symbols/class.a8", QColor(74, 158, 224));
        return icon;
    }
    case FfiSymbolKind::Struct: {
        static const QIcon icon = maskIcon(":/ui/icons/symbols/struct.a8", QColor(53, 182, 147));
        return icon;
    }
    case FfiSymbolKind::Enum: {
        static const QIcon icon = maskIcon(":/ui/icons/symbols/enum.a8", QColor(210, 164, 76));
        return icon;
    }
    case FfiSymbolKind::Interface: {
        static const QIcon icon = maskIcon(":/ui/icons/symbols/interface.a8", QColor(166, 114, 217));
        return icon;
    }
    case FfiSymbolKind::Method: {
        static const QIcon icon = maskIcon(":/ui/icons/symbols/method.a8", QColor(74, 158, 224));
        return icon;
    }
    case FfiSymbolKind::Function: {
        static const QIcon icon = maskIcon(":/ui/icons/symbols/function.a8", QColor(199, 125, 209));
        return icon;
    }
    case FfiSymbolKind::Field: {
        static const QIcon icon = maskIcon(":/ui/icons/symbols/field.a8", QColor(79, 178, 224));
        return icon;
    }
    case FfiSymbolKind::Constant: {
        static const QIcon icon = maskIcon(":/ui/icons/symbols/constant.a8", QColor(224, 132, 74));
        return icon;
    }
    case FfiSymbolKind::Property: {
        static const QIcon icon = maskIcon(":/ui/icons/symbols/property.a8", QColor(79, 192, 160));
        return icon;
    }
    case FfiSymbolKind::Constructor: {
        static const QIcon icon = maskIcon(":/ui/icons/symbols/constructor.a8", QColor(224, 86, 107));
        return icon;
    }
    case FfiSymbolKind::EnumMember: {
        static const QIcon icon = maskIcon(":/ui/icons/symbols/enum_member.a8", QColor(210, 164, 76));
        return icon;
    }
    }
    return {};
}

QIcon symbolCategoryIcon(FfiSymbolCategory category)
{
    switch (category) {
    case FfiSymbolCategory::Constants: {
        static const QIcon icon =
          maskIcon(":/ui/icons/symbols/category_constants.a8", QColor(224, 132, 74));
        return icon;
    }
    case FfiSymbolCategory::Fields: {
        static const QIcon icon = maskIcon(":/ui/icons/symbols/category_fields.a8", QColor(79, 178, 224));
        return icon;
    }
    case FfiSymbolCategory::Properties: {
        static const QIcon icon =
          maskIcon(":/ui/icons/symbols/category_properties.a8", QColor(79, 192, 160));
        return icon;
    }
    case FfiSymbolCategory::Constructors: {
        static const QIcon icon =
          maskIcon(":/ui/icons/symbols/category_constructors.a8", QColor(224, 86, 107));
        return icon;
    }
    case FfiSymbolCategory::Methods: {
        static const QIcon icon = maskIcon(":/ui/icons/symbols/category_methods.a8", QColor(74, 158, 224));
        return icon;
    }
    case FfiSymbolCategory::NestedTypes: {
        static const QIcon icon =
          maskIcon(":/ui/icons/symbols/category_nested_types.a8", QColor(166, 114, 217));
        return icon;
    }
    case FfiSymbolCategory::Other: {
        static const QIcon icon = maskIcon(":/ui/icons/symbols/category_other.a8", QColor(150, 150, 150));
        return icon;
    }
    }
    return {};
}

} // namespace ui_shell
