(function_signature) @function.around
(method_signature) @function.around
(abstract_method_signature) @function.around

(abstract_class_declaration body: (_) @class.inside) @class.around
(interface_declaration body: (_) @class.inside) @class.around
(enum_declaration body: (_) @class.inside) @class.around
(type_alias_declaration) @class.around

(type_parameters (_) @parameter.inside)
(type_arguments (_) @parameter.inside)
