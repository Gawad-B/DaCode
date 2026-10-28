(function_definition body: (_) @function.inside) @function.around

(struct_specifier body: (_) @class.inside) @class.around
(union_specifier body: (_) @class.inside) @class.around
(enum_specifier body: (_) @class.inside) @class.around

(parameter_list (_) @parameter.inside)
(argument_list (_) @parameter.inside)

(comment) @comment.around
