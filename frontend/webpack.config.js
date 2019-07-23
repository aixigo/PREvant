const path = require('path');
const MiniCssExtractPlugin = require('mini-css-extract-plugin');
const webpack = require( 'webpack' );
const WebpackNotifierPlugin = require('webpack-notifier');
const VueLoaderPlugin = require('vue-loader/lib/plugin');

const devMode = process.env.NODE_ENV !== 'production'

module.exports = {

    entry: {
        'init': './src/app.js'
    },

    output: {
        path: path.resolve(__dirname, './target'),
        publicPath: '/',
        filename: 'app.js'
    },

    plugins: [
        new webpack.ProvidePlugin({
            'jQuery': 'jquery',
            '$': 'jquery',
            Popper: 'popper.js'
        }),
        new VueLoaderPlugin(),
        new MiniCssExtractPlugin({
            // Options similar to the same options in webpackOptions.output
            // both options are optional
            filename: '[name].css'
        }),
        ...(process.argv.some(arg => arg === '--notify')
            ? [new WebpackNotifierPlugin()]
            : [])
    ],

    module: {
        rules: [
            {
                test: /.vue$/,
                loader: 'vue-loader'
            },
            {
                test: /\.js$/,
                loader: 'babel-loader',
                include: [path.join(__dirname, 'src'), path.join(__dirname, 'node_modules')]
            },
            {
                test: /\.s?css$/,
                use: [
                    devMode ? 'style-loader' : MiniCssExtractPlugin.loader,
                    'css-loader?sourceMap',
                    'postcss-loader?sourceMap'
                ]
            },
            {
                test: /\.scss$/,
                use: [
                    'sass-loader?sourceMap'
                ]
            },
            {
                test: /\.(gif|jpe?g|png|ttf|woff2?|svg|eot|otf)(\?.*)?$/,
                loader: 'file-loader',
                options: {
                    name: '[name].[ext]?[hash]'
                }
            }
        ]
    },
    resolve: {
        alias: {
            'vue$': 'vue/dist/vue.esm.js',
            'popper.js$': 'popper.js/dist/umd/popper.js'
        },
        extensions: ['*', '.js', '.vue', '.json']
    },
    devServer: {
        port: 9001,
        proxy: {
            '/api': {
                target: 'http://localhost:8000'
            },
            '/openapi.yaml': {
               target: 'http://localhost:8000'
            }
        }
    },
    performance: {
        hints: false
    },
    devtool: '#cheap-source-map',
    mode: !devMode ? 'production' : 'development'
};
