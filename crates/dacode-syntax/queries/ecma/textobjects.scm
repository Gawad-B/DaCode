(function_declaration body: (_) @function.inside) @function.around
(function_expression body: (_) @function.inside) @function.around
(generator_function_declaration body: (_) @function.inside) @function.around
(generator_function body: (_) @function.inside) @function.around
(arrow_function body: (_) @function.inside) @function.around
(method_definition body: (_) @function.inside) @function.around

(class_declaration body: (_) @class.inside) @class.around
(class body: (_) @class.inside) @class.around

(formal_parameters (_) @parameter.inside)
(arguments (_) @parameter.inside)

(comment) @comment.around
