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
            Popper: ['popper.js', 'default']
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
                test: /\.m?js$/,
                loader: 'babel-loader',
                exclude: /node_modules/,
                options: {
                    presets: [
                        '@babel/preset-env'
                    ]
                }
            },
            {
                test: /\.(gif|jpe?g|png|ttf|woff2?|svg|eot|otf)(\?.*)?$/,
                loader: 'file-loader',
                options: {
                    name: '[name].[ext]?[hash]'
                }
            },
            {
                test: /.vue$/,
                loader: 'vue-loader'
            }
        ]
    },
    resolve: {
        alias: {
            'vue$': 'vue/dist/vue.esm.js'
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
    devtool: '#eval-source-map',
    mode: !devMode ? 'production' : 'development'
};
