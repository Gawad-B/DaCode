(lambda_expression body: (_) @function.inside) @function.around

(class_specifier body: (_) @class.inside) @class.around

(template_argument_list (_) @parameter.inside)
(template_parameter_list (_) @parameter.inside)
