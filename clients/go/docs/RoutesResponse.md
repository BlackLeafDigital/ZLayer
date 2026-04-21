# RoutesResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Routes** | [**[]RouteInfo**](RouteInfo.md) | Route details. | 
**Total** | **int32** | Total number of routes. | 

## Methods

### NewRoutesResponse

`func NewRoutesResponse(routes []RouteInfo, total int32, ) *RoutesResponse`

NewRoutesResponse instantiates a new RoutesResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewRoutesResponseWithDefaults

`func NewRoutesResponseWithDefaults() *RoutesResponse`

NewRoutesResponseWithDefaults instantiates a new RoutesResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetRoutes

`func (o *RoutesResponse) GetRoutes() []RouteInfo`

GetRoutes returns the Routes field if non-nil, zero value otherwise.

### GetRoutesOk

`func (o *RoutesResponse) GetRoutesOk() (*[]RouteInfo, bool)`

GetRoutesOk returns a tuple with the Routes field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRoutes

`func (o *RoutesResponse) SetRoutes(v []RouteInfo)`

SetRoutes sets Routes field to given value.


### GetTotal

`func (o *RoutesResponse) GetTotal() int32`

GetTotal returns the Total field if non-nil, zero value otherwise.

### GetTotalOk

`func (o *RoutesResponse) GetTotalOk() (*int32, bool)`

GetTotalOk returns a tuple with the Total field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTotal

`func (o *RoutesResponse) SetTotal(v int32)`

SetTotal sets Total field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


