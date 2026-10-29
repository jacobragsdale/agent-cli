// The orders API on gin with swaggo: swag init (in the Dockerfile) writes the
// docs package from these comments, and gin-swagger serves it at its README's
// route, /swagger/*any.
package main

import (
	"net/http"
	"strconv"
	"sync"

	_ "contoso/orders/docs"

	"github.com/gin-gonic/gin"
	swaggerFiles "github.com/swaggo/files"
	ginSwagger "github.com/swaggo/gin-swagger"
)

type Line struct {
	Sku string `json:"sku" binding:"required"`
	Qty int    `json:"qty" binding:"required"`
}

type OrderIn struct {
	CustomerID int     `json:"customer_id" binding:"required"`
	Lines      []Line  `json:"lines" binding:"required"`
	Note       *string `json:"note,omitempty"`
}

type Order struct {
	ID         int     `json:"id"`
	CustomerID int     `json:"customer_id"`
	Status     string  `json:"status" enums:"open,shipped,cancelled,on_hold"`
	Lines      []Line  `json:"lines"`
	Note       *string `json:"note,omitempty"`
}

type Problem struct {
	Detail string `json:"detail"`
}

var (
	mu     sync.Mutex
	orders = []Order{
		{1, 101, "open", []Line{{"SKU-001", 1}}, nil},
		{2, 102, "open", []Line{{"SKU-002", 2}}, nil},
		{3, 103, "open", []Line{{"SKU-003", 3}}, nil},
	}
)

// listOrders godoc
// @Summary List orders
// @Param   status query string false "Only orders in this status" Enums(open, shipped, cancelled, on_hold)
// @Produce json
// @Success 200 {array} Order
// @Router  /orders [get]
func listOrders(c *gin.Context) {
	mu.Lock()
	defer mu.Unlock()
	status := c.Query("status")
	out := []Order{}
	for _, o := range orders {
		if status == "" || o.Status == status {
			out = append(out, o)
		}
	}
	c.JSON(http.StatusOK, out)
}

// getOrder godoc
// @Summary Get an order
// @Param   id path int true "Order id"
// @Produce json
// @Success 200 {object} Order
// @Failure 404 {object} Problem
// @Router  /orders/{id} [get]
func getOrder(c *gin.Context) {
	mu.Lock()
	defer mu.Unlock()
	id, _ := strconv.Atoi(c.Param("id"))
	for _, o := range orders {
		if o.ID == id {
			c.JSON(http.StatusOK, o)
			return
		}
	}
	c.JSON(http.StatusNotFound, Problem{"order " + c.Param("id") + " not found"})
}

// createOrder godoc
// @Summary Create an order
// @Accept  json
// @Produce json
// @Param   order body OrderIn true "The order"
// @Success 201 {object} Order
// @Failure 400 {object} Problem
// @Router  /orders [post]
func createOrder(c *gin.Context) {
	var in OrderIn
	if err := c.ShouldBindJSON(&in); err != nil {
		c.JSON(http.StatusBadRequest, Problem{err.Error()})
		return
	}
	mu.Lock()
	defer mu.Unlock()
	o := Order{len(orders) + 1, in.CustomerID, "open", in.Lines, in.Note}
	orders = append(orders, o)
	c.JSON(http.StatusCreated, o)
}

// @title   Contoso Orders API
// @version 1.0
func main() {
	r := gin.Default()
	r.GET("/orders", listOrders)
	r.GET("/orders/:id", getOrder)
	r.POST("/orders", createOrder)
	r.GET("/swagger/*any", ginSwagger.WrapHandler(swaggerFiles.Handler))
	r.Run(":8080")
}
