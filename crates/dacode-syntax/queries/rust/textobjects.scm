(function_item body: (_) @function.inside) @function.around
(function_signature_item) @function.around
(closure_expression body: (_) @function.inside) @function.around

(struct_item body: (_) @class.inside) @class.around
(struct_item) @class.around
(enum_item body: (_) @class.inside) @class.around
(union_item body: (_) @class.inside) @class.around
(trait_item body: (_) @class.inside) @class.around
(impl_item body: (_) @class.inside) @class.around
(type_item) @class.around

(parameters (_) @parameter.inside)
(closure_parameters (_) @parameter.inside)
(type_parameters (_) @parameter.inside)
(arguments (_) @parameter.inside)
(type_arguments (_) @parameter.inside)

(line_comment) @comment.around
(block_comment) @comment.around
