(function_definition body: (_) @function.inside) @function.around
(lambda body: (_) @function.inside) @function.around

(class_definition body: (_) @class.inside) @class.around

(parameters (_) @parameter.inside)
(lambda_parameters (_) @parameter.inside)
(argument_list (_) @parameter.inside)

(comment) @comment.around
