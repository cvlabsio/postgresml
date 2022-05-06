-- This example trains models on the sklean digits dataset
-- which is a copy of the test set of the UCI ML hand-written digits datasets
-- https://archive.ics.uci.edu/ml/datasets/Optical+Recognition+of+Handwritten+Digits
--
-- This demonstrates using a table with a single array feature column
-- for classification.
--
-- The final result after a few seconds of training is not terrible. Maybe not perfect
-- enough for mission critical applications, but it's telling how quickly "off the shelf" 
-- solutions can solve problems these days.

-- Exit on error (psql)
\set ON_ERROR_STOP true

SELECT pgml.load_dataset('digits');

-- view the dataset
SELECT left(image::text, 40) || ',...}', target FROM pgml.digits LIMIT 10;

-- train a simple model to classify the data
SELECT * FROM pgml.train('Handwritten Digit Image Classifier', 'classification', 'pgml.digits', 'target');

-- check out the predictions
SELECT target, pgml.predict('Handwritten Digit Image Classifier', image) AS prediction
FROM pgml.digits 
LIMIT 10;

-- After a project has been trained, ommited parameters will be reused from previous training runs
-- In these examples we'll reuse the training data snapshots from the initial call.
-- linear models
SELECT * FROM pgml.train('Handwritten Digit Image Classifier', algorithm => 'ridge');
SELECT * FROM pgml.train('Handwritten Digit Image Classifier', algorithm => 'stochastic_gradient_descent');
SELECT * FROM pgml.train('Handwritten Digit Image Classifier', algorithm => 'perceptron');
SELECT * FROM pgml.train('Handwritten Digit Image Classifier', algorithm => 'passive_aggressive');
-- support vector machines
SELECT * FROM pgml.train('Handwritten Digit Image Classifier', algorithm => 'svm');
SELECT * FROM pgml.train('Handwritten Digit Image Classifier', algorithm => 'nu_svm');
SELECT * FROM pgml.train('Handwritten Digit Image Classifier', algorithm => 'linear_svm');
-- ensembles
SELECT * FROM pgml.train('Handwritten Digit Image Classifier', algorithm => 'ada_boost');
SELECT * FROM pgml.train('Handwritten Digit Image Classifier', algorithm => 'bagging');
SELECT * FROM pgml.train('Handwritten Digit Image Classifier', algorithm => 'extra_trees', hyperparams => '{"n_estimators": 10}');
SELECT * FROM pgml.train('Handwritten Digit Image Classifier', algorithm => 'gradient_boosting_trees', hyperparams => '{"n_estimators": 10}');
-- Histogram Gradient Boosting is too expensive for normal tests on even a toy dataset
-- SELECT * FROM pgml.train('Handwritten Digit Image Classifier', algorithm => 'hist_gradient_boosting', hyperparams => '{"max_iter": 2}');
SELECT * FROM pgml.train('Handwritten Digit Image Classifier', algorithm => 'random_forest', hyperparams => '{"n_estimators": 10}');
-- other
-- Gaussian Process is too expensive for normal tests on even a toy dataset
-- SELECT * FROM pgml.train('Handwritten Digit Image Classifier', algorithm => 'gaussian_process', hyperparams => '{"max_iter_predict": 100, "warm_start": true}');
SELECT * FROM pgml.train('Handwritten Digit Image Classifier', algorithm => 'xgboost');
SELECT * FROM pgml.train('Handwritten Digit Image Classifier', algorithm => 'xgboost_random_forest');


-- check out all that hard work
SELECT trained_models.* FROM pgml.trained_models 
JOIN pgml.models on models.id = trained_models.id
ORDER BY models.metrics->>'f1' DESC LIMIT 5;

-- deploy the random_forest model for prediction use
SELECT * FROM pgml.deploy('Handwritten Digit Image Classifier', 'most_recent', 'random_forest');
-- check out that throughput
SELECT * FROM pgml.deployed_models ORDER BY deployed_at DESC LIMIT 5;

-- do a hyperparam search on your favorite algorithm
SELECT pgml.train(
    'Handwritten Digit Image Classifier', 
    algorithm => 'svm', 
    hyperparams => '{"random_state": 0}',
    search => 'grid', 
    search_params => '{
        "kernel": ["linear", "poly", "sigmoid"], 
        "shrinking": [true, false]
    }'
);

-- TODO SELECT pgml.hypertune(100, 'Handwritten Digit Image Classifier', 'classification', 'pgml.digits', 'target', 'gradient_boosted_trees');

-- deploy the "best" model for prediction use
SELECT * FROM pgml.deploy('Handwritten Digit Image Classifier', 'best_score');
SELECT * FROM pgml.deploy('Handwritten Digit Image Classifier', 'most_recent');
SELECT * FROM pgml.deploy('Handwritten Digit Image Classifier', 'rollback');
SELECT * FROM pgml.deploy('Handwritten Digit Image Classifier', 'best_score', 'svm');

-- check out the improved predictions
SELECT target, pgml.predict('Handwritten Digit Image Classifier', image) AS prediction
FROM pgml.digits 
LIMIT 10;